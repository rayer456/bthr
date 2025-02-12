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

use crate::signal::{BthrSignal, GuiSignal, TaskSignal};


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

    notifications_stream_acquired_at: Option<SystemTime>,
    last_heart_rate_ping: Option<SystemTime>,

    // Create separate struct including these 2 fields
    last_connection_failure: Option<Instant>,
    should_reconnect: Option<String>, // Should be used to connect asap after start rescanning process


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
            notifications_stream_acquired_at: None,
            last_heart_rate_ping: None,
            active_device_name: String::new(),
            last_connection_failure: None,
            should_reconnect: None,
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
        self.notifications_stream_acquired_at = None; 
        self.last_heart_rate_ping = None;
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

    async fn check_for_heart_rate_ping(&mut self) {
        // Check if a HeartRatePing is expected at this time or not.
        // If a ping is expected, but we don't receive one past a set timeout threshold, 
        // then we make the assumption that the connecting task is stuck waiting for BT data input. 
        // In that case the connecting task should be ended. 
        // TODO: or probably don't just end it, at least not after 10 seconds, maybe make it configurable
        
        let Some(notification_time) = self.notifications_stream_acquired_at else { return; };
        let Ok(notification_time_elapsed) = notification_time.elapsed() else { return; };
        if self.last_heart_rate_ping.is_none() && notification_time_elapsed > Duration::from_secs(10) {
            println!("HR STUCK");
            self.end_connecting_task().await;
            return;
        }

        let Some(last_hr_ping_time) = self.last_heart_rate_ping else { return; };
        let Ok(last_hr_ping_elapsed) = last_hr_ping_time.elapsed() else { return; };
        if last_hr_ping_elapsed > Duration::from_secs(10) {
            println!("HR STUCK");
            self.end_connecting_task().await;
            return;
        }
    }

    async fn generic_connection_failure_retry(&mut self) {
        // Determines if connecting task should be restarted if current restarting period
        // is below a certain time threshold.
        // This function will set the correct variables for another
        // function to restart the connecting task.

        let now = Instant::now();
        let last_failure = self.last_connection_failure.unwrap_or(now);
        if self.last_connection_failure.is_none() {
            self.last_connection_failure = Some(last_failure);
        }

        let failing_for = now.duration_since(last_failure);

        // TODO: make time threshold configurable
        if failing_for > Duration::from_secs(30) {
            self.last_connection_failure = None;
            println!("Reached reconnection threshold"); // Do something in GUI here too
            return;
        }

        println!("Connection reset for {:?}", failing_for);

        let active_device_name = self.active_device_name.clone();
        self.should_reconnect = Some(active_device_name); // Checked during the main loop
    }

    async fn reconnect_when_device_found(&mut self) {
        let Some(ref device) = self.should_reconnect else { return; };
        let device = device.clone();

        if let Some(_) = find_peri_by_name(&device, &self.peris).await {
            self.start_connecting_task(&device).await;

            // After starting connecting task again, reset this to avoid an infinite loop.
            // Technically not an infinite loop but this function will be called again sooner than we can connect to the device.
            // TODO: if this works just remove the other should_reconnect resets
            self.should_reconnect = None;
        }
    }

    async fn notification_stream_acquired(&mut self) {
        self.notifications_stream_acquired_at = Some(SystemTime::now());
        let _ = self.tx_to_gui.send(BthrSignal::ActiveDevice(self.active_device_name.clone())).await;
        self.end_scanning_task().await;
        self.last_connection_failure = None;
    }

    fn heart_rate_ping_received(&mut self) {
        self.last_heart_rate_ping = Some(SystemTime::now());
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
                TaskSignal::HeartRatePing => self.heart_rate_ping_received(),
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

        self.check_for_heart_rate_ping().await;
        self.reconnect_when_device_found().await;
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

async fn find_peri_by_name<'a>(clicked_peri_name: &'a String, peris: &'a Vec<PlatformPeripheral>) -> Option<&'a PlatformPeripheral> {
    for peri in peris {
        let Some(peri_name) = get_peripheral_name(peri).await else { continue; };
        if *clicked_peri_name == peri_name {
            println!("Found peri by name");
            return Some(peri);
        }
    }
    println!("Didn't find peri by name");
    None
}

async fn get_peripheral_name(peripheral: &PlatformPeripheral) -> Option<String> {
    let Ok(Some(properties)) = peripheral.properties().await else { return None; };

    properties.local_name
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

    // Refactor this? Put it somewhere else?
    let mut i = 0;
    let peripheral = loop {
        if let Some(peripheral) = find_peri_by_name(&name, &peris).await {
            break peripheral;
        }

        sleep(Duration::from_secs(1)).await;
        if i == 4 {
            let _ = tx_to_bthr.send(TaskSignal::PeripheralNotFound(name)).await;
            return;
        }

        i += 1;
        continue;
    };

    let peripheral = peripheral.clone();
    if !try_connect_to_peripheral(&peripheral).await {
        let _ = tx_to_bthr.send(TaskSignal::ConnectionFailed).await;
        return;
    }

    // Connected past this point

    let discovery_res = peripheral.discover_services().await;
    if discovery_res.is_err() {
        disconnect_from_peri(&peripheral).await;
        let _ = tx_to_bthr.send(TaskSignal::DiscoveringServicesFailed).await;
        return;
    }

    // Rewrite this with iterator again
    let mut found_characteristic_opt: Option<Characteristic> = None;
    let mut found_char = false;
    for characteristic in peripheral.characteristics() {
        if characteristic.uuid == HEART_RATE_MEASUREMENT_UUID && characteristic.properties.contains(CharPropFlags::NOTIFY) {
            found_characteristic_opt = Some(characteristic);
            found_char= true;
            println!("FOUND CHAR");

            break;
        }
    }

    if !found_char {
        disconnect_from_peri(&peripheral).await;
        let _ = tx_to_bthr.send(TaskSignal::HrCharNotFound).await;
        return;
    }

    let found_characteristic = found_characteristic_opt.unwrap();

    println!("Subscribing to characteristic {:?}", found_characteristic.uuid);

    // Try to subscribe a couple times, then fail
    for i in 0..5 {
        // Unsub first
        match peripheral.unsubscribe(&found_characteristic).await {
            Ok(_) => println!("Unsubscribed successfully!"),
            _ => println!("Failed to unsubscribe, might have already been subscribed..."),
        };
        
        // Try subscribing
        match peripheral.subscribe(&found_characteristic).await {
            Ok(_) => break,
            Err(_) if i < 4 => (),
            _ => {
                disconnect_from_peri(&peripheral).await;
                let _ = tx_to_bthr.send(TaskSignal::CharSubscriptionFailed).await;
                return;
            },
        };
        sleep(Duration::from_millis(200)).await;
    }

    let Ok(mut notifications_stream) = peripheral.notifications().await else {
        disconnect_from_peri(&peripheral).await;
        let _ = tx_to_bthr.send(TaskSignal::NotificationStreamFailed).await;
        return;
    };

    let _ = tx_to_bthr.send(TaskSignal::NotificationStreamAcquired).await;

    let mut i = 1;
    loop {
        tokio::select! {
            Some(data) = notifications_stream.next() => {
                let Some(hr) = data.value.get(1) else { continue; };
                // println!("heartbeat: {hr}");

                let _ = tx_to_gui.send(BthrSignal::HeartRate {
                    heart_rate: *hr,
                }).await;

                let _ = tx_to_bthr.send(TaskSignal::HeartRatePing).await;

                /* if i == 10 {
                } */
                i += 1;
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
