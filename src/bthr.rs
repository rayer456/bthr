use std::ops::Deref;
use std::sync::mpsc::Receiver as StdReceiver;
use std::time::{Duration, SystemTime};

use eframe::egui::Id;
use tokio::net::windows::named_pipe;
use tokio::{signal, spawn};
use tokio::sync::mpsc::{Receiver as TokioReceiver, Sender as TokioSender};
use futures::StreamExt;
use anyhow::{bail, Result};

use tokio::task::JoinHandle;
use tokio::time::sleep;
use uuid::Uuid;
use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral as PlatformPeripheral};

use crate::signal::{BthrSignal, GuiSignal, ScanSignal};


const HEART_RATE_MEASUREMENT_UUID: Uuid = Uuid::from_u128(0x00002a3700001000800000805f9b34fb);


pub struct BthrManager {
    tx_to_gui: TokioSender<BthrSignal>,
    rx_to_bthr: TokioReceiver<ScanSignal>,
    tx_to_bthr: TokioSender<ScanSignal>,
    rx_to_gui: StdReceiver<GuiSignal>,
    peris: Vec<PlatformPeripheral>,
    current_scanning_task: Option<JoinHandle<()>>,
    current_connecting_task: Option<JoinHandle<()>>,
    notifications_stream_acquired_at: Option<SystemTime>,
    last_heart_rate_ping: Option<SystemTime>,
}

impl BthrManager {
    pub fn new(tx_to_gui: TokioSender<BthrSignal>, rx_to_gui: StdReceiver<GuiSignal>) -> Self {
        let (tx_to_bthr, rx_to_bthr) = tokio::sync::mpsc::channel(128);
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
        }
    }

    fn start_scanning_task(&mut self) {
        // If scanning task exists end it first.
        self.end_scanning_task();

        println!("Starting scanning task...");
        let scan_handle = spawn(scan_for_peripherals(self.tx_to_gui.clone(), self.tx_to_bthr.clone()));
        self.current_scanning_task = Some(scan_handle);
    }

    fn end_scanning_task(&self) {
        println!("Ending scanning task...");
        if let Some(task) = &self.current_scanning_task {
            task.abort();
        }
    }

    fn start_connecting_task(&mut self, name: &String) {
        // End scanning task and connecting task
        self.end_connecting_task();
        self.end_scanning_task();

        // Use a ping to really test if task is dead

        println!("Starting connecting task...");
        let peris_clone = self.peris.clone();
        let tx_to_gui_clone = self.tx_to_gui.clone();
        let tx_to_bthr_clone = self.tx_to_bthr.clone();
        let name_clone = name.clone();
        let connect_handle = spawn(async {
            connect_peri(name_clone, peris_clone, tx_to_gui_clone, tx_to_bthr_clone).await;
        });

        self.current_connecting_task = Some(connect_handle);
    }

    fn end_connecting_task(&mut self) {
        println!("Ending connecting task...");

        // Important to reset these fields.
        self.notifications_stream_acquired_at = None; 
        self.last_heart_rate_ping = None;

        if let Some(connecting_task) = &self.current_connecting_task {
            connecting_task.abort();
            if !connecting_task.is_finished() {
                println!("Existing connecting task was NOT aborted.");
            } else {
                println!("Existing connecting task was aborted.");
            }
        }
    }

    fn check_for_heart_rate_ping(&mut self) {
        // Check if a HeartRatePing is expected at this time or not.
        // If a ping is expected, but we don't receive one past a set timeout threshold, then we make the assumption that the connecting task is stuck waiting for BT data input. In that case the connecting task should be ended.
        
        let Some(notification_time) = self.notifications_stream_acquired_at else { return; };
        let Ok(notification_time_elapsed) = notification_time.elapsed() else { return; };
        if self.last_heart_rate_ping.is_none() && notification_time_elapsed > Duration::from_secs(5) {
            // Assume task is stuck
            self.end_connecting_task();
            return;
        }

        let Some(last_hr_ping_time) = self.last_heart_rate_ping else { return; };
        let Ok(last_hr_ping_elapsed) = last_hr_ping_time.elapsed() else { return; };
        if last_hr_ping_elapsed > Duration::from_secs(5) {
            // Assume task is stuck
            self.end_connecting_task();
            return;
        } else {
            println!("last ping: {:?}", last_hr_ping_elapsed);
        }
    }

    async fn read_channels(&mut self) {
        // Acts as a router/controller
        // Checks all receiving channels

        // Probably won't need to return Signal since this will only be called in the main loop and not by other methods.

        // ConnectDevice and StopScanning should stop the scanning task.

        

        if let Ok(signal) = self.rx_to_gui.try_recv() {
            match signal {
                GuiSignal::StartScanning => self.start_scanning_task(),
                GuiSignal::ConnectDevice(name) => self.start_connecting_task(&name),
                GuiSignal::StopScanning => self.end_scanning_task(),
                _ => ()
            };
        }
    
        if let Ok(signal) = self.rx_to_bthr.try_recv() {
            match signal {
                ScanSignal::Peripherals(peris) => self.peris = peris,
                ScanSignal::NotificationStreamAcquired => self.notifications_stream_acquired_at = Some(SystemTime::now()),
                ScanSignal::HeartRatePing => {
                    self.last_heart_rate_ping = Some(SystemTime::now());
                    println!("HeartRatePing received!");
                },
            };
        }

        self.check_for_heart_rate_ping();
    

    }

    pub async fn main_loop(&mut self) {
        loop {
            self.read_channels().await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    

}

async fn scan_for_peripherals(tx_to_gui: TokioSender<BthrSignal>, tx_to_bthr: TokioSender<ScanSignal>) {
    let Ok(manager) = Manager::new().await else { return; };
    let Ok(adapter_list) = manager.adapters().await else { return; };

    // Send EMPTY signal, wait some time before scanning for adapters again
    if adapter_list.is_empty() {
        eprintln!("No Bluetooth adapters found");
    }

    for adapter in adapter_list.iter() {
        println!("{}", adapter.adapter_info().await.unwrap_or("No name adapter".to_string()))
    }

    let adapter = adapter_list.iter().nth(0).expect("No Bluetooth adapter found.");

    // Send SCAN signal
    // GUI should respond to this instead of "assuming" the scan started
    tx_to_gui.send(BthrSignal::StartScan).await;


    loop {
        adapter
            .start_scan(ScanFilter::default())
            .await
            .expect("Can't scan BLE adapter for connected devices..."); // don't use expect() here but show something in GUI and try looping

        // TODO: what happens with multiple bluetooth adapters?

        // continue reading channel to know when to 
        // 1. connect a device (user clicked button)
        // 2. stop scanning
        /* match self.read_channel().await {
            GuiSignal::ConnectDevice(name) => self.connect_peri(name).await,
            GuiSignal::StopScanning => return,
            _ => (),
        } */

        // peripherals contains a list of p's discovered so far
        // may contain items that are no longer available (so keep that in mind when trying to connect)
        let Ok(mut peripherals) = adapter.peripherals().await else { return; };

        // Attempting to connect to a peri from the list can fail so be ready to scan again 
        // for devices and make them available to connect to again

        // TODO Try this: Send discovered devices to GUI and show them
        let mut peris = vec![];
        for per in peripherals.iter() {
            let Ok(Some(properties)) = per.properties().await else { continue; };
            let Some(name) = properties.local_name else { continue; }; // use unwrap or and give default name
            peris.push(name);
        }

        tx_to_bthr.send(ScanSignal::Peripherals(peripherals)).await;

        let _ = tx_to_gui.send(BthrSignal::DiscoveredPeripherals(peris)).await;

        
        println!("scanning");
        println!("\n");

        // Sleep here as we don't want to scan for devices a billion times per second
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn find_peri_by_name<'a>(clicked_peri_name: &'a String, peris: &'a Vec<PlatformPeripheral>) -> Option<&'a PlatformPeripheral> {
    for peri in peris {
        println!("{:?}", peri);
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

async fn connect_peri(name: String, peris: Vec<PlatformPeripheral>, tx_to_gui: TokioSender<BthrSignal>, tx_to_bthr: TokioSender<ScanSignal>) -> Result<()>{
    let Some(peripheral) = find_peri_by_name(&name, &peris).await else { bail!("connecting failed: no name"); };

    let peripheral = peripheral.clone();

    if !peripheral.is_connected().await.unwrap() {
        // Connect if we aren't already connected.
        if let Err(err) = peripheral.connect().await {
            bail!("Error connecting to peripheral, skipping: {}", err);
        }
    }

    if peripheral.is_connected().await.unwrap() {
        println!("Discover peripheral {} services...", name);
        peripheral.discover_services().await.unwrap();
        for characteristic in peripheral.characteristics() {
            // println!("Checking characteristic {:?}", characteristic);
            // Subscribe to notifications from the characteristic with the selected
            // UUID.
            if characteristic.uuid == HEART_RATE_MEASUREMENT_UUID
                && characteristic.properties.contains(CharPropFlags::NOTIFY)
            {
                println!("Subscribing to characteristic {:?}", characteristic.uuid);
                peripheral.subscribe(&characteristic).await.unwrap(); // DONT SUBSCRIBE TWICE!!!!!!!!
                

                // Process while the BLE connection is not broken or stopped.
                let mut notifications_stream = peripheral.notifications().await.unwrap();

                let _ = tx_to_bthr.send(ScanSignal::NotificationStreamAcquired).await;

                while let Some(data) = notifications_stream.next().await {
                    let hr = *data.value.get(1).unwrap();
                    println!("heartbeat: {hr}");

                    let _ = tx_to_gui.send(BthrSignal::HeartRate {
                        heart_rate: hr,
                    }).await;

                    let _ = tx_to_bthr.send(ScanSignal::HeartRatePing).await;
                }
                
            }
        }
        println!("Disconnecting from peripheral {:?}...", name);
        peripheral.disconnect().await.unwrap();
    }
    bail!("lol failed");
}
