use std::ops::Deref;
use std::sync::mpsc::Receiver as StdReceiver;
use std::time::Duration;

use tokio::net::windows::named_pipe;
use tokio::{signal, spawn};
use tokio::sync::mpsc::{Receiver as TokioReceiver, Sender as TokioSender};
use futures::StreamExt;
use anyhow::Result;

use tokio::task::JoinHandle;
use tokio::time::sleep;
use uuid::Uuid;
use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral as PlatformPeripheral};

use crate::signal::{BthrSignal, GuiSignal, ScanSignal};


const DEVICE_NAME: &'static str = "COROS PACE Pro B69E81";
const HEART_RATE_MEASUREMENT_UUID: Uuid = Uuid::from_u128(0x00002a3700001000800000805f9b34fb);


pub struct BthrManager {
    tx_to_gui: TokioSender<BthrSignal>,
    rx_to_bthr: TokioReceiver<ScanSignal>,
    tx_to_bthr: TokioSender<ScanSignal>,
    rx_to_gui: StdReceiver<GuiSignal>,
    peris: Vec<PlatformPeripheral>,
    current_scanning_task: Option<JoinHandle<()>>,
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
        }
    }

    fn stop_scanning_task(&self) {
        if let Some(task) = &self.current_scanning_task {
            task.abort();

            // test
            if self.current_scanning_task.is_some() {
                println!("Scanning task still exists!");
            } else {
                println!("Scanning task was aborted!");
            }
        }
    }

    fn start_scanning_task(&mut self) {
        let scan_handle = spawn(scan_for_peripherals(self.tx_to_gui.clone(), self.tx_to_bthr.clone()));
        self.current_scanning_task = Some(scan_handle);
    }

    fn start_connecting_task(&mut self, name: &String) {
        println!("from bthr thread: {} was clicked!", name);
        connect_peri(name, peris)
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
                GuiSignal::StopScanning => self.stop_scanning_task(),
                _ => ()
            };
        }
    
        if let Some(ref mut rx_to_bthr) = self.rx_to_bthr {
            let Ok(signal) = rx_to_bthr.try_recv() else { return; };
            match signal {
                ScanSignal::Peripherals(peris) => self.peris = peris,
                _ => (),
            };
       }

        

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

        tx_to_bthr.send(ScanSignal::Peripherals(peripherals));

        let _ = tx_to_gui.send(BthrSignal::DiscoveredPeripherals(peris)).await;

        
        println!("\n");

        // Sleep here as we don't want to scan for devices a billion times per second
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn find_peri_by_name(clicked_peri_name: &String, peris: &Vec<PlatformPeripheral>) -> Option<&PlatformPeripheral> {
    for peri in peris {
        let peri_name = get_peripheral_name(peri).await.unwrap();
        if *clicked_peri_name == peri_name {
            return Some(peri);
        }
    }
    None
}

async fn connect_peri(name: String, peris: Vec<PlatformPeripheral>) {
    let Some(peripheral) = find_peri_by_name(&name, &peris).await else { return; };

    let peripheral = peripheral.clone();

    
    if !peripheral.is_connected().await.unwrap() {
        // Connect if we aren't already connected.
        if let Err(err) = peripheral.connect().await {
            eprintln!("Error connecting to peripheral, skipping: {}", err);
            return;
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
                peripheral.subscribe(&characteristic).await.unwrap();
                
                // Process while the BLE connection is not broken or stopped.
                while let Some(data) = peripheral.notifications().await.unwrap().next().await {
                    /* println!(
                        "Received data from {:?} [{:?}]: {:?}",
                        local_name, data.uuid, data.value
                    ); */
                    let hr = *data.value.get(1).unwrap();
                    println!("heartbeat: {hr}");
                    let _ = self.tx_to_gui.send(BthrSignal::HeartRate { 
                        heart_rate: hr,
                    }).await;
                }
            }
        }
        println!("Disconnecting from peripheral {:?}...", name);
        peripheral.disconnect().await.unwrap();
    }
}




async fn get_peripheral_name(peripheral: &PlatformPeripheral) -> Option<String> {
    let Ok(Some(properties)) = peripheral.properties().await else { return None; };

    properties.local_name
}