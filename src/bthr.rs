use std::process::exit;
use std::sync::mpsc::Receiver as StdReceiver;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::FontData;
use tokio::{spawn, sync};
use tokio::sync::mpsc::{Receiver as TokioReceiver, Sender as TokioSender};
use futures::{FutureExt, StreamExt};

use tokio::task::JoinHandle;
use tokio::time::sleep;
use uuid::Uuid;
use btleplug::api::{Central, CharPropFlags, Characteristic, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::{Manager, Peripheral as PlatformPeripheral};

use crate::signal::{BthrSignal, GuiSignal, TaskSignal};


const HEART_RATE_MEASUREMENT_UUID: Uuid = Uuid::from_u128(0x00002a3700001000800000805f9b34fb);
// For Testing
// const BATTERY_LEVEL_UUID: Uuid = Uuid::from_u128(0x00002a1900001000800000805f9b34fb);


pub struct BthrManager {
    tx_to_gui: TokioSender<BthrSignal>,
    rx_to_bthr: TokioReceiver<TaskSignal>,
    tx_to_bthr: TokioSender<TaskSignal>,
    rx_to_gui: StdReceiver<GuiSignal>,
    tx_for_dc: Option<TokioSender<i8>>,
    peris: Vec<PlatformPeripheral>,
    current_scanning_task: Option<JoinHandle<()>>,
    current_connecting_task: Option<JoinHandle<()>>,
    notifications_stream_acquired_at: Option<SystemTime>,
    last_heart_rate_ping: Option<SystemTime>,
    active_device_name: String,
    last_connection_failure: Option<Instant>,
    should_reconnect: Option<String>, // Should be used to connect asap after start rescanning process
}

impl BthrManager {
    pub fn new(tx_to_gui: TokioSender<BthrSignal>, rx_to_gui: StdReceiver<GuiSignal>) -> Self {
        let (tx_to_bthr, rx_to_bthr) = tokio::sync::mpsc::channel(64);
        BthrManager {
            tx_to_gui,
            rx_to_bthr,
            tx_to_bthr,
            rx_to_gui,
            tx_for_dc: None,
            peris: vec![],
            current_scanning_task: None,
            current_connecting_task: None,
            notifications_stream_acquired_at: None,
            last_heart_rate_ping: None,
            active_device_name: String::new(),
            last_connection_failure: None,
            should_reconnect: None,
        }
    }

    async fn start_scanning_task(&mut self) {
        // If scanning task exists end it first.
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

        let (tx_for_dc, rx_for_dc) = tokio::sync::mpsc::channel(5);
        self.tx_for_dc = Some(tx_for_dc);
        let peris_clone = self.peris.clone();
        let tx_to_gui_clone = self.tx_to_gui.clone();
        let tx_to_bthr_clone = self.tx_to_bthr.clone();
        let name_clone = name.clone();
        self.active_device_name = name.to_string();

        let connect_handle = spawn(connect_peri(name_clone, peris_clone, tx_to_gui_clone, tx_to_bthr_clone, rx_for_dc));

        self.current_connecting_task = Some(connect_handle);
    }

    async fn end_connecting_task(&mut self) {
        // Important to reset these fields.
        self.notifications_stream_acquired_at = None; 
        self.last_heart_rate_ping = None;
        self.active_device_name.clear();

        if let Some(task) = &self.current_connecting_task {
            if task.is_finished() { 
                println!("connecting task already finished");
                self.current_connecting_task = None;
                return;
            };
            
            if let Some(tx) = &self.tx_for_dc {
                let _ = tx.send(1).await;
            }

            println!("Ending connecting task by sending signal");
            println!("Shouldn't need to call abort()")

            

            // TODO maybe wait until task is actually finished before moving on when using a signal
            /* task.abort();
            let _ = self.tx_to_gui.send(BthrSignal::DeviceDisconnected("Connecting task forced to end".to_string())).await; */
        }
    }

    async fn check_for_heart_rate_ping(&mut self) {
        // Check if a HeartRatePing is expected at this time or not.
        // If a ping is expected, but we don't receive one past a set timeout threshold, then we make the assumption that the connecting task is stuck waiting for BT data input. In that case the connecting task should be ended.
        
        let Some(notification_time) = self.notifications_stream_acquired_at else { return; };
        let Ok(notification_time_elapsed) = notification_time.elapsed() else { return; };
        if self.last_heart_rate_ping.is_none() && notification_time_elapsed > Duration::from_secs(10) {
            // Assume task is stuck
            println!("HR STUCK");
            self.end_connecting_task().await;
            return;
        }

        let Some(last_hr_ping_time) = self.last_heart_rate_ping else { return; };
        let Ok(last_hr_ping_elapsed) = last_hr_ping_time.elapsed() else { return; };
        if last_hr_ping_elapsed > Duration::from_secs(10) {
            // Assume task is stuck
            println!("HR STUCK");
            self.end_connecting_task().await;
            return;
        } else {
            // println!("last ping: {:?}", last_hr_ping_elapsed);
        }
    }

    async fn generic_connection_failure_retry(&mut self) {
        // Determines if connecting task should be restarted based on how long it thinks
        // it has been restarting. This function will set the correct variables for another
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
            println!("Reached recent connection time threshold"); // Do something in GUI here too
            return;
        }

        println!("Connection retry threshold not reached: been failing for {:?}", failing_for);
        // try connecting again
        let active_device_name = self.active_device_name.clone();

        // Taking note here for reconnect_when_device_found()
        self.should_reconnect = Some(active_device_name);
    }

    async fn reconnect_when_device_found(&mut self) {
        // Start connecting process after scanning discovered the past device.
        let Some(ref device) = self.should_reconnect else {
            return;
        };
        let device = device.clone();

        if let Some(_) = find_peri_by_name(&device, &self.peris).await {
            self.start_connecting_task(&device).await;

            // After starting connecting task again, reset this to avoid an infinite loop.
            // Technically not an infinite loop but this function will be called again sooner than we can connect to the device.
            // TODO: if this works just remove the other should_reconnect resets
            self.should_reconnect = None;
        }
    }


    async fn gui_peri_not_found(&mut self, peri_name: String) {
        // Should probably try again
        println!("peri {peri_name} not found after trying for a while");
        self.generic_connection_failure_retry().await;
    }
    
    async fn gui_connection_failed(&mut self) {
        println!("connection failed");
        self.generic_connection_failure_retry().await;
    }

    async fn gui_service_discovery_failed(&mut self) {
        println!("service discovery failed");
        let _ = self.tx_to_gui.send(BthrSignal::DeviceDisconnected("Discovery failed".to_string())).await;
        self.generic_connection_failure_retry().await;
    }

    async fn gui_failed_to_find_hr_char(&mut self) {
        println!("failed to read hr char");
        let _ = self.tx_to_gui.send(BthrSignal::DeviceDisconnected("Failed to find HR char".to_string())).await;
        self.generic_connection_failure_retry().await;
    }

    async fn gui_failed_to_sub_to_char(&mut self) {
        println!("failed to subscribe");
        let _ = self.tx_to_gui.send(BthrSignal::DeviceDisconnected("Failed to sub to char".to_string())).await;
        self.generic_connection_failure_retry().await;
    }

    async fn gui_notif_stream_failed(&mut self) {
        println!("notif stream failed");
        let _ = self.tx_to_gui.send(BthrSignal::DeviceDisconnected("Notification stream failed".to_string())).await;
        self.generic_connection_failure_retry().await;
    }

    async fn gui_peri_disconnected(&mut self) {
        // This is only called when the connecting_task function reaches its end.
        // Not when the peri disconnects for any other reason

        // Should probably try to reconnect if this happens.
        // Don't know when this happens

        println!("peri disconnected");
        let _ = self.tx_to_gui.send(BthrSignal::DeviceDisconnected("Notification stream ended".to_string())).await;

        // Turn off for testing
        //self.generic_connection_failure_retry().await;
    }

    async fn adapter_not_found(&mut self) {
        // In case no adapter was found. (after task is killed)
        eprintln!("No Bluetooth adapters found");
    }

    async fn failed_scan(&mut self) {
        // When starting a scan fails...
        println!("Can't scan BLE adapter for connected devices...");
    }

    fn show_new_peris(&mut self, peris: Vec<PlatformPeripheral>) {
        self.peris = peris;
    }

    async fn read_channels(&mut self) {
        // Acts as a router/controller
        // Checks all receiving channels

        // Probably won't need to return Signal since this will only be called in the main loop and not by other methods.

        // ConnectDevice and StopScanning should stop the scanning task.


        if let Ok(signal) = self.rx_to_gui.try_recv() {
            // If user chooses to stop scanning (maybe because continuous failure)
            // Don't try to reconnect to an old device when scanning again.
            // If this isn't done the program could try to reconnect to an old device even when the user doesn't want to connect to it.
            self.should_reconnect = None;

            match signal {
                GuiSignal::StartScanning => self.start_scanning_task().await,
                GuiSignal::ConnectDevice(name) => self.start_connecting_task(&name).await,
                GuiSignal::StopScanning => self.end_scanning_task().await,
                GuiSignal::DisconnectDevice => self.end_connecting_task().await,
            };
        }
    
        if let Ok(signal) = self.rx_to_bthr.try_recv() {
            match signal {
                TaskSignal::PeripheralsFound(peris) => self.show_new_peris(peris),
                TaskSignal::NotificationStreamAcquired => {
                    println!("noti stream acquired");
                    self.notifications_stream_acquired_at = Some(SystemTime::now());
                    
                    // TODO: When connected, should probably end scanning task
                    let _ = self.tx_to_gui.send(BthrSignal::ActiveDevice(self.active_device_name.clone())).await; // Implicitly means process of connecting is stopped
                    self.should_reconnect = None;
                    self.last_connection_failure = None;
                },
                TaskSignal::HeartRatePing => {
                    self.last_heart_rate_ping = Some(SystemTime::now());
                },
                TaskSignal::PeripheralNotFound(peri_name) => self.gui_peri_not_found(peri_name).await,
                TaskSignal::ConnectionFailed => self.gui_connection_failed().await,
                TaskSignal::DiscoveringServicesFailed => self.gui_service_discovery_failed().await,
                TaskSignal::HrCharNotFound => self.gui_failed_to_find_hr_char().await,
                TaskSignal::CharSubscriptionFailed => self.gui_failed_to_sub_to_char().await,
                TaskSignal::NotificationStreamFailed => self.gui_notif_stream_failed().await,
                TaskSignal::PeripheralDisconnected => self.gui_peri_disconnected().await,
                TaskSignal::AdapterNotFound => self.adapter_not_found().await,
                TaskSignal::FailedScan => self.failed_scan().await,
            };
        }

        self.check_for_heart_rate_ping().await;
        self.reconnect_when_device_found().await;
    }

    pub async fn main_loop(&mut self) {
        loop {
            self.read_channels().await;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
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

    // GUI should respond to this instead of "assuming" the scan started
    let _ = tx_to_gui.send(BthrSignal::ScanStarted).await;

    let Ok(_) = adapter.start_scan(ScanFilter::default()).await else {
        let _ = tx_to_bthr.send(TaskSignal::FailedScan).await;
        return;
    };
    loop {

        // TODO: what happens with multiple bluetooth adapters?

        let Ok(peripherals) = adapter.peripherals().await else { return; };

        // Attempting to connect to a peri from the list can fail so be ready to scan again 
        // for devices and make them available to connect to again

        let mut peris = vec![];
        for per in peripherals.iter() {
            let Ok(Some(properties)) = per.properties().await else { continue; };
            let Some(name) = properties.local_name else { continue; }; // use unwrap or and give default name
            peris.push(name);
        }

        let _ = tx_to_bthr.send(TaskSignal::PeripheralsFound(peripherals)).await;

        let _ = tx_to_gui.send(BthrSignal::DiscoveredPeripherals(peris)).await;

        
       /*  println!("scanning");
        println!("\n"); */

        // Sleep here as we don't want to scan for devices a billion times per second
        tokio::time::sleep(Duration::from_secs(1)).await;

    }
}

async fn find_peri_by_name<'a>(clicked_peri_name: &'a String, peris: &'a Vec<PlatformPeripheral>) -> Option<&'a PlatformPeripheral> {
    for peri in peris {
        let Some(peri_name) = get_peripheral_name(peri).await else { continue; };
        if *clicked_peri_name == peri_name {
            println!("Found peri");
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

async fn connect_peri(name: String, peris: Vec<PlatformPeripheral>, tx_to_gui: TokioSender<BthrSignal>, tx_to_bthr: TokioSender<TaskSignal>, mut rx_for_dc: TokioReceiver<i8>) {

    // Sometimes existing peripheral can't be found even though it should exist?
    // After 5 attempts not found, fail.
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

    let mut found_characteristic_opt: Option<Characteristic> = None;
    let mut found_char = false;
    for characteristic in peripheral.characteristics() {
        // println!("uuid: {}", characteristic.uuid);
        if characteristic.uuid == HEART_RATE_MEASUREMENT_UUID && characteristic.properties.contains(CharPropFlags::NOTIFY) {
            found_characteristic_opt = Some(characteristic);
            found_char= true;
            println!("FOUND CHAR");

            break;
        }
    }

    // 00002a37-0000-1000-8000-00805f9b34fb

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
        if let Some(data) = notifications_stream.next().await {
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

        // TODO: https://stackoverflow.com/questions/64084955/how-to-remotely-shut-down-running-tasks-with-tokio
        println!("waiting...");
        match rx_for_dc.try_recv() {
            Ok(_) => {
                disconnect_from_peri(&peripheral).await;
                // TODO add a separate signal for this to make clear user dc'ed
                println!("dc from user");
                let _ = tx_to_bthr.send(TaskSignal::PeripheralDisconnected).await;
                return;
            },
            Err(_) => (), 
        }
        
        /* // If loop escapes: send disconnect signal
        disconnect_from_peri(&peripheral).await;
        let _ = tx_to_bthr.send(TaskSignal::PeripheralDisconnected).await; */
    }

}

async fn disconnect_from_peri(peripheral: &PlatformPeripheral) {
    let _ = peripheral.disconnect().await;
}
