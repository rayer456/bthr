use std::sync::mpsc::Receiver as StdReceiver;
use std::time::{Duration, Instant, SystemTime};

use tokio::spawn;
use tokio::sync::mpsc::{Receiver as TokioReceiver, Sender as TokioSender};
use futures::StreamExt;

use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use btleplug::api::{Central, CharPropFlags, Characteristic, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::{Manager, Peripheral as PlatformPeripheral};

use crate::helpers;
use crate::reconnector::{self, Reconnector};
use crate::signal::{BthrSignal, GuiSignal, TaskSignal};
use crate::pulse::Pulse;


const HEART_RATE_MEASUREMENT_UUID: Uuid = Uuid::from_u128(0x00002a3700001000800000805f9b34fb);


pub struct BthrManager {
    tx_to_gui: TokioSender<BthrSignal>,
    rx_to_bthr: TokioReceiver<TaskSignal>,
    tx_to_bthr: TokioSender<TaskSignal>,
    rx_to_gui: StdReceiver<GuiSignal>,

    peris: Vec<PlatformPeripheral>,
    current_scanning_task: Option<JoinHandle<()>>,
    current_connecting_task: Option<JoinHandle<()>>,
    active_device_name: String,

    pulse: Pulse,

    reconnector: Reconnector,


    cancellation_token: CancellationToken, // Used to remotely tell connecting task to abort
}

impl BthrManager {
    pub fn new(tx_to_gui: TokioSender<BthrSignal>, rx_to_gui: StdReceiver<GuiSignal>) -> Self {
        let (tx_to_bthr, rx_to_bthr) = tokio::sync::mpsc::channel(64);
        BthrManager {
            tx_to_gui,
            rx_to_bthr,
            tx_to_bthr,
            rx_to_gui,
            peris: vec![],
            current_scanning_task: None,
            current_connecting_task: None,
            active_device_name: String::new(),
            pulse: Pulse::new(),
            reconnector: Reconnector::new(),
            cancellation_token: CancellationToken::new(),
        }
    }

    async fn start_scanning_task(&mut self) {
        self.end_scanning_task().await;

        println!("Starting scanning task...");
        let scan_handle = spawn(scan_for_peripherals(self.tx_to_gui.clone(), self.tx_to_bthr.clone()));
        self.current_scanning_task = Some(scan_handle);
    }

    async fn end_scanning_task(&mut self) {
        if let Some(task) = &self.current_scanning_task {
            if task.is_finished() {
                println!("scanning task already finished");
                self.current_scanning_task = None;
                return;
            };

            println!("Ending scanning task...");
            task.abort();
            let _ = self.tx_to_gui.send(BthrSignal::ScanStopped).await;
        } else {
            println!("No scanning task available");
        }
        
    }

    async fn start_connecting_task(&mut self, name: &String) {

        self.end_connecting_task().await;

        println!("Starting connecting task...");

        let _ = self.tx_to_gui.send(BthrSignal::Connecting).await;

        let peris_clone = self.peris.clone();
        let tx_to_gui_clone = self.tx_to_gui.clone();
        let tx_to_bthr_clone = self.tx_to_bthr.clone();
        let name_clone = name.clone();
        self.active_device_name = name.to_string();
        self.cancellation_token = CancellationToken::new();
        let cloned_token = self.cancellation_token.clone();

        let connect_handle = spawn(connect_peri(
            name_clone, 
            peris_clone, 
            tx_to_gui_clone,
            tx_to_bthr_clone, 
            cloned_token,
        ));

        self.current_connecting_task = Some(connect_handle);
    }

    async fn end_connecting_task(&mut self) {
        // Important to reset these fields.
        self.pulse.reset_notifications_stream_acquire_time();
        self.pulse.reset_last_pulse();
        self.active_device_name.clear();

        if let Some(task) = &self.current_connecting_task {
            if task.is_finished() { 
                self.current_connecting_task = None;
                return;
            };

            self.cancellation_token.cancel();
            println!("Sent cancel signal, task should end");

            // Task doesn't properly cancel without sleep here
            tokio::time::sleep(Duration::from_millis(200)).await; 
        }
    }

    async fn check_pulse(&mut self) {
        // In that case the connecting task should be ended. 
        // TODO: or probably don't just end it, at least not after 10 seconds, maybe make it configurable
        
        if self.pulse.is_stuck() {
            self.end_connecting_task().await;
            return;
        }
    }

    async fn generic_connection_failure_retry(&mut self) {
        self.reconnector.prepare_for_reconnection(self.active_device_name.clone());
    }

    async fn reconnect_if_needed(&mut self) {
        if self.reconnector.should_reconnect(&self.peris).await {
            self.start_connecting_task(&self.active_device_name.clone()).await;
        }
    }

    async fn notification_stream_acquired(&mut self) {
        // Basically means: established a proper connection here, reading stream now

        self.pulse.notif_stream_acquired();
        let _ = self.tx_to_gui.send(BthrSignal::ActiveDevice(self.active_device_name.clone())).await;
        self.end_scanning_task().await;
        self.reconnector.last_connection_failure = None;
    }

    async fn gui_peri_not_found(&mut self, peri_name: String) {
        println!("peri {peri_name} not found after trying for a while");
        self.generic_connection_failure_retry().await;
    }
    
    async fn gui_connection_failed(&mut self) {
        println!("connection failed");
        self.generic_connection_failure_retry().await;
    }

    async fn gui_service_discovery_failed(&mut self) {
        println!("service discovery failed");

        let _ = self.tx_to_gui.send(BthrSignal::DeviceDisconnected { 
            reason: "Service discovery failed".to_string(), 
            was_connecting: true, 
        }).await;

        self.generic_connection_failure_retry().await;
    }

    async fn gui_failed_to_find_hr_char(&mut self) {
        println!("failed to read hr char");

        let _ = self.tx_to_gui.send(BthrSignal::DeviceDisconnected { 
            reason: "Failed to find HR char".to_string(), 
            was_connecting: true, 
        }).await;

        self.generic_connection_failure_retry().await;
    }

    async fn gui_failed_to_sub_to_char(&mut self) {
        println!("failed to subscribe");

        let _ = self.tx_to_gui.send(BthrSignal::DeviceDisconnected { 
            reason: "Failed to sub to char".to_string(), 
            was_connecting: true, 
        }).await;

        self.generic_connection_failure_retry().await;
    }

    async fn gui_notif_stream_failed(&mut self) {
        println!("notif stream failed");

        let _ = self.tx_to_gui.send(BthrSignal::DeviceDisconnected { 
            reason: "Notification stream failed".to_string(), 
            was_connecting: true, 
        }).await;

        self.generic_connection_failure_retry().await;
    }

    async fn gui_peri_disconnected(&mut self) {
        // Only called when peri disconnects by cancellation token (end_connecting_task() is called)

        let _ = self.tx_to_gui.send(BthrSignal::DeviceDisconnected { 
            reason: "Cancellation token cancelled".to_string(), 
            was_connecting: false, 
        }).await;
    }

    async fn adapter_not_found(&mut self) {
        eprintln!("No Bluetooth adapters found");
    }

    async fn failed_scan(&mut self) {
        // When starting a scan fails...
        println!("Can't scan BLE adapter for connected devices...");
    }

    fn set_new_peris(&mut self, peris: Vec<PlatformPeripheral>) {
        self.peris = peris;
    }

    async fn read_channels(&mut self) {
        // Acts as a router/controller
        // Checks all receiving channels and other conditions

        if let Ok(signal) = self.rx_to_gui.try_recv() {
            match signal {
                GuiSignal::StartScanning => self.start_scanning_task().await,
                GuiSignal::ConnectDevice(name) => self.start_connecting_task(&name).await,
                GuiSignal::StopScanning => self.end_scanning_task().await,
                GuiSignal::DisconnectDevice => self.end_connecting_task().await,
            };
        }
    
        if let Ok(signal) = self.rx_to_bthr.try_recv() {
            match signal {
                TaskSignal::PeripheralsFound(peris) => self.set_new_peris(peris),
                TaskSignal::NotificationStreamAcquired => self.notification_stream_acquired().await,
                TaskSignal::Pulse => self.pulse.pulse_received(),
                TaskSignal::PeripheralDisconnected => self.gui_peri_disconnected().await,
                TaskSignal::PeripheralNotFound(peri_name) => self.gui_peri_not_found(peri_name).await,

                // Unusual signals
                TaskSignal::ConnectionFailed => self.gui_connection_failed().await,
                TaskSignal::DiscoveringServicesFailed => self.gui_service_discovery_failed().await,
                TaskSignal::HrCharNotFound => self.gui_failed_to_find_hr_char().await,
                TaskSignal::CharSubscriptionFailed => self.gui_failed_to_sub_to_char().await,
                TaskSignal::NotificationStreamFailed => self.gui_notif_stream_failed().await,
                TaskSignal::AdapterNotFound => self.adapter_not_found().await,
                TaskSignal::FailedScan => self.failed_scan().await,
            };
        }

        self.check_pulse().await;
        self.reconnect_if_needed().await;
    }
}

pub async fn main_loop(mut bthr_manager: BthrManager) {
    loop {
        bthr_manager.read_channels().await;
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn scan_for_peripherals(tx_to_gui: TokioSender<BthrSignal>, tx_to_bthr: TokioSender<TaskSignal>) {

    let Ok(manager) = Manager::new().await else {
        let _ = tx_to_bthr.send(TaskSignal::AdapterNotFound).await;
        return;
    };

    let Ok(adapter_list) = manager.adapters().await else { 
        let _ = tx_to_bthr.send(TaskSignal::AdapterNotFound).await;
        return; 
    };

    let Some(adapter) = adapter_list.iter().nth(0) else {
        let _ = tx_to_bthr.send(TaskSignal::AdapterNotFound).await;
        return;
    };

    // Signal to GUI scan started
    let _ = tx_to_gui.send(BthrSignal::ScanStarted).await;

    let Ok(_) = adapter.start_scan(ScanFilter::default()).await else {
        let _ = tx_to_bthr.send(TaskSignal::FailedScan).await;
        return;
    };
    loop {
        // TODO: what happens with multiple bluetooth adapters?

        let Ok(peripherals) = adapter.peripherals().await else { return; };

        let mut peris = vec![];
        for per in peripherals.iter() {
            let Ok(Some(properties)) = per.properties().await else { continue; };
            let Some(name) = properties.local_name else { continue; }; 
            // TODO use unwrap or and give default name
            peris.push(name);
        }

        let _ = tx_to_bthr.send(TaskSignal::PeripheralsFound(peripherals)).await; // Sending peri
        let _ = tx_to_gui.send(BthrSignal::DiscoveredPeripherals(peris)).await; // Sending peri name

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn try_connect_to_peripheral(peripheral: &PlatformPeripheral) -> bool {
    let Ok(peri_is_connected) = peripheral.is_connected().await else {
        return false;
    };

    if peri_is_connected {
        return true;
    }

    if peripheral.connect().await.is_ok() {
        return true;
    }

    false
}

async fn connect_peri(name: String, peris: Vec<PlatformPeripheral>, tx_to_gui: TokioSender<BthrSignal>, tx_to_bthr: TokioSender<TaskSignal>, cancellation_token: CancellationToken) {

    // let Some(peripheral) = find_peri_on_timeout(name.clone(), peris).await else {
    //     let _ = tx_to_bthr.send(TaskSignal::PeripheralNotFound(name)).await;
    //     return;
    // };

    let Some(mut peripheral) = helpers::find_peri_by_name(&name, &peris).await else {
        let _ = tx_to_bthr.send(TaskSignal::PeripheralNotFound(name)).await;
        return;
    };

    if !try_connect_to_peripheral(&peripheral).await {
        let _ = tx_to_bthr.send(TaskSignal::ConnectionFailed).await;
        return;
    }

    // Connected past this point

    if let Err(_) = peripheral.discover_services().await {
        disconnect_from_peri(&peripheral).await;
        let _ = tx_to_bthr.send(TaskSignal::DiscoveringServicesFailed).await;
        return;
    }

    let found_characteristic_opt = peripheral.characteristics()
        .into_iter()
        .find(|char| char.uuid == HEART_RATE_MEASUREMENT_UUID && char.properties.contains(CharPropFlags::NOTIFY));

    let Some(found_characteristic) = found_characteristic_opt else {
        disconnect_from_peri(&peripheral).await;
        let _ = tx_to_bthr.send(TaskSignal::HrCharNotFound).await;
        return;
    };

    println!("FOUND CHAR");
    println!("Subscribing to characteristic {:?}", found_characteristic.uuid);

    if !try_subscribing_to_char(&mut peripheral, &found_characteristic).await {
        disconnect_from_peri(&peripheral).await;
        let _ = tx_to_bthr.send(TaskSignal::CharSubscriptionFailed).await;
        return;
    }

    let Ok(mut notifications_stream) = peripheral.notifications().await else {
        disconnect_from_peri(&peripheral).await;
        let _ = tx_to_bthr.send(TaskSignal::NotificationStreamFailed).await;
        return;
    };

    // Important signal
    let _ = tx_to_bthr.send(TaskSignal::NotificationStreamAcquired).await;


    // Maybe split this part into a separate function too? Kinda unnecessary
    // Loop used for testing
    // let mut i = 1; 
    loop {
        tokio::select! {
            Some(data) = notifications_stream.next() => {
                let Some(hr) = data.value.get(1) else { continue; };
                // println!("heartbeat: {hr}");

                let _ = tx_to_gui.send(BthrSignal::HeartRate {
                    heart_rate: *hr,
                }).await;

                let _ = tx_to_bthr.send(TaskSignal::Pulse).await;

                /* if i == 10 {
                } */
                // i += 1;
            }
            _ = cancellation_token.cancelled() => {
                disconnect_from_peri(&peripheral).await;
                // TODO add a separate signal for this to make clear user dc'ed
                println!("dc from user");
                let _ = tx_to_bthr.send(TaskSignal::PeripheralDisconnected).await;
                return;
            }
        }
    }
}

async fn disconnect_from_peri(peripheral: &PlatformPeripheral) {
    let _ = peripheral.disconnect().await;
}


// Can probably be removed since useless
// Keeping here just in case
async fn find_peri_on_timeout(name: String, peris: Vec<PlatformPeripheral>) -> Option<PlatformPeripheral> {
    let mut i = 0;
    let peripheral = loop {
        if let Some(peripheral) = helpers::find_peri_by_name(&name, &peris).await {
            break peripheral;
        }

        sleep(Duration::from_secs(1)).await;
        if i == 4 {
            return None;
        }

        i += 1;
        continue;
    };

    Some(peripheral)
}

async fn try_subscribing_to_char(
    // Try subscribing a couple times with timeout, then fail

    peripheral: &mut PlatformPeripheral, 
    characteristic: &Characteristic) -> bool {
    for i in 0..5 {
        // Unsub first
        match peripheral.unsubscribe(characteristic).await {
            Ok(_) => println!("Unsubscribed successfully!"),
            _ => println!("Failed to unsubscribe, might have already been subscribed..."),
        };
        
        // Try subscribing
        match peripheral.subscribe(characteristic).await {
            Ok(_) => return true,
            Err(_) if i < 4 => {
                sleep(Duration::from_millis(200)).await;
                continue;
            },
            _ => {
                return false;
            },
        };
    }

    false
}