use std::ops::Deref;
use std::sync::mpsc::Receiver as StdReceiver;
use std::time::Duration;

use tokio::net::windows::named_pipe;
use tokio::sync::mpsc::Sender as TokioSender;
use futures::StreamExt;
use anyhow::Result;

use uuid::Uuid;
use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral as PlatformPeripheral};

use crate::signal::{BthrSignal, GuiSignal};


const DEVICE_NAME: &'static str = "COROS PACE Pro B69E81";
const HEART_RATE_MEASUREMENT_UUID: Uuid = Uuid::from_u128(0x00002a3700001000800000805f9b34fb);


pub struct BthrManager {
    tx_to_gui: TokioSender<BthrSignal>,
    rx_to_gui: StdReceiver<GuiSignal>,
}

impl BthrManager {
    pub fn new(tx_to_gui: TokioSender<BthrSignal>, rx_to_gui: StdReceiver<GuiSignal>) -> Self {
        BthrManager {
            tx_to_gui,
            rx_to_gui,
        }
    }

    fn read_channel(&mut self) {
        let Ok(info) = self.rx_to_gui.try_recv() else { return; };
        match info {
            GuiSignal::ConnectDevice(name) => println!("from bthr thread: {name} was clicked!"),
        }
    }

    async fn get_peripherals(&mut self) -> Result<Vec<PlatformPeripheral>> {
        let manager = Manager::new().await?;
        let adapter_list = manager.adapters().await?;
    
        // Send EMPTY signal, wait some time before scanning for adapters again
        if adapter_list.is_empty() {
            eprintln!("No Bluetooth adapters found");
        }
    
        for adapter in adapter_list.iter() {
            println!("{}", adapter.adapter_info().await.unwrap_or("No name adapter".to_string()))
        }
    
        let adapter = adapter_list.iter().nth(0).expect("No Bluetooth adapter found.");
    
        // Send SCAN signal
        self.tx_to_gui.send(BthrSignal::StartScan).await;
    
        // Maybe put loop in own function?
        let mut peripherals: Vec<PlatformPeripheral> = vec![]; 
        loop {
            adapter
                .start_scan(ScanFilter::default())
                .await
                .expect("Can't scan BLE adapter for connected devices..."); // don't use expect() here but show something in GUI and try looping
    
            // TODO: what happens with multiple bluetooth adapters?
    
            self.read_channel();
    
            // peripherals contains a list of p's discovered so far
            // may contain items that are no longer available (so keep that in mind when trying to connect)
            peripherals = adapter.peripherals().await?;
    
            // Attempting to connect to a peri from the list can fail so be ready to scan again 
            // for devices and make them available to connect to again
    
            // TODO Try this: Send discovered devices to GUI and show them
            let mut peris = vec![];
            for per in peripherals.iter() {
                let Some(properties) = per.properties().await? else { continue; };
                let Some(name) = properties.local_name else { continue; };
                peris.push(name);
            }
    
            let _ = self.tx_to_gui.send(BthrSignal::DiscoveredPeripherals(peris)).await;
                /* if name == DEVICE_NAME && peripherals.len() >= 2 { // for testing return as soon as Watch and one other device is found.
                    
                    // return Ok(peripherals)
                } */
                
    
            println!("\n");
    
            // Sleep here as we don't want to scan for devices a billion times per second
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    pub async fn run(&mut self) -> Result<()> {

        let mut peripherals = self.get_peripherals().await?;
    
        if peripherals.is_empty() {
            // Send empty list to GUI, but keep trying to call get_peripherals()
            eprintln!("->>> BLE peripheral devices were not found, sorry. Exiting...");
        } else {
            // User will choose via GUI but this is testing
            for peripheral in peripherals.iter() {
                let properties = peripheral.properties().await?;
                let local_name = properties
                    .unwrap()
                    .local_name
                    .unwrap_or(String::from("(peripheral name unknown)"));
    
                // Skip unwanted peripherals
                if !local_name.contains(DEVICE_NAME) {
                    continue;
                }
    
                println!("Found matching peripheral {:?}...", &local_name);
    
                if !peripheral.is_connected().await? {
                    // Connect if we aren't already connected.
                    if let Err(err) = peripheral.connect().await {
                        eprintln!("Error connecting to peripheral, skipping: {}", err);
                        continue;
                    }
                }
    
                if peripheral.is_connected().await? {
                    println!("Discover peripheral {} services...", local_name);
                    peripheral.discover_services().await?;
                    for characteristic in peripheral.characteristics() {
                        // println!("Checking characteristic {:?}", characteristic);
                        // Subscribe to notifications from the characteristic with the selected
                        // UUID.
                        if characteristic.uuid == HEART_RATE_MEASUREMENT_UUID
                            && characteristic.properties.contains(CharPropFlags::NOTIFY)
                        {
                            println!("Subscribing to characteristic {:?}", characteristic.uuid);
                            peripheral.subscribe(&characteristic).await?;
                            
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
                    println!("Disconnecting from peripheral {:?}...", local_name);
                    peripheral.disconnect().await?;
                }
            }
        }
        Ok(())
    }
}






async fn get_peripheral_name(peripheral: &PlatformPeripheral) -> Option<String> {
    let Ok(Some(properties)) = peripheral.properties().await else { return None; };

    properties.local_name
}