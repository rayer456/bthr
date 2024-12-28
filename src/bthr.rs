use std::ops::Deref;
use std::sync::mpsc::Receiver as StdReceiver;
use std::time::Duration;

use tokio::net::windows::named_pipe;
use tokio::sync::mpsc::Sender as TokioSender;
use futures::StreamExt;
use anyhow::Result;

use tokio::time::sleep;
use uuid::Uuid;
use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral as PlatformPeripheral};

use crate::signal::{BthrSignal, GuiSignal};


const DEVICE_NAME: &'static str = "COROS PACE Pro B69E81";
const HEART_RATE_MEASUREMENT_UUID: Uuid = Uuid::from_u128(0x00002a3700001000800000805f9b34fb);


pub struct BthrManager {
    tx_to_gui: TokioSender<BthrSignal>,
    rx_to_gui: StdReceiver<GuiSignal>,
    peris: Vec<PlatformPeripheral>,
}

impl BthrManager {
    pub fn new(tx_to_gui: TokioSender<BthrSignal>, rx_to_gui: StdReceiver<GuiSignal>) -> Self {
        BthrManager {
            tx_to_gui,
            rx_to_gui,
            peris: vec![],
        }
    }

    async fn read_channel(&mut self) -> GuiSignal {
        // Acts as a router/controller

        let Ok(signal) = self.rx_to_gui.try_recv() else { return GuiSignal::Empty; };
        match signal {
            GuiSignal::ConnectDevice(ref name) => println!("from bthr thread: {} was clicked!", name),
            GuiSignal::StartScanning => Box::pin(self.scan_for_peripherals()).await,
            _ => ()
        };

        signal
    }

    pub async fn main_loop(&mut self) {
        loop {
            self.read_channel().await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn scan_for_peripherals(&mut self) {
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
        self.tx_to_gui.send(BthrSignal::StartScan).await;
    

        loop {
            adapter
                .start_scan(ScanFilter::default())
                .await
                .expect("Can't scan BLE adapter for connected devices..."); // don't use expect() here but show something in GUI and try looping
    
            // TODO: what happens with multiple bluetooth adapters?
    
            // continue reading channel to know when to 
            // 1. connect a device (user clicked button)
            // 2. stop scanning
            match self.read_channel().await {
                GuiSignal::ConnectDevice(name) => self.connect_peri(name).await,
                GuiSignal::StopScanning => return,
                _ => (),
            }
    
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
    
            self.peris = peripherals;
            let _ = self.tx_to_gui.send(BthrSignal::DiscoveredPeripherals(peris)).await;
                /* if name == DEVICE_NAME && peripherals.len() >= 2 { // for testing return as soon as Watch and one other device is found.
                    
                    // return Ok(peripherals)
                } */
                
    
            println!("\n");
    
            // Sleep here as we don't want to scan for devices a billion times per second
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn find_peri_by_name(&mut self, clicked_peri_name: &String) -> Option<&PlatformPeripheral> {
        for peri in &self.peris {
            let peri_name = get_peripheral_name(peri).await.unwrap();
            if *clicked_peri_name == peri_name {
                return Some(peri);
            }
        }
        None
    }

    async fn connect_peri(&mut self, name: String) {
        let Some(peripheral) = self.find_peri_by_name(&name).await else { return; };

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
}




async fn get_peripheral_name(peripheral: &PlatformPeripheral) -> Option<String> {
    let Ok(Some(properties)) = peripheral.properties().await else { return None; };

    properties.local_name
}