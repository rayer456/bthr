use std::ops::Deref;
use std::time::Duration;

use tokio::net::windows::named_pipe;
use tokio::sync::mpsc::Sender;
use futures::StreamExt;
use anyhow::Result;

use uuid::Uuid;
use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral as PlatformPeripheral};

use crate::bthr_info::BthrInfo;


const DEVICE_NAME: &'static str = "COROS PACE Pro B69E81";
const HEART_RATE_MEASUREMENT_UUID: Uuid = Uuid::from_u128(0x00002a3700001000800000805f9b34fb);


// maybe put tx Sender as parameter?
pub async fn get_peripherals(tx: Sender<BthrInfo>) -> Result<Vec<PlatformPeripheral>> {
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
    println!("Starting scan...");

    // Maybe put loop in own function?
    let mut peripherals: Vec<PlatformPeripheral> = vec![]; 
    loop {
        adapter
            .start_scan(ScanFilter::default())
            .await
            .expect("Can't scan BLE adapter for connected devices..."); // don't use expect() here but show something in GUI and try looping

        // TODO: what happens with multiple bluetooth adapters?

        // Sleep here as we don't want to scan for devices a billion times per second
        tokio::time::sleep(Duration::from_secs(1)).await;

        // peripherals contains a list of p's discovered so far
        // may contain items that are no longer available (so keep that in mind when trying to connect)
        peripherals = adapter.peripherals().await?;

        // Attempting to connect to a peri from the list can fail so be ready to scan again 
        // for devices and make them available to connect to again

        // TODO Try this: Send discovered devices to GUI and show them
        for per in peripherals.iter() {
            let properties = per.properties().await?;
            let name_opt = properties.unwrap().local_name;
            if let Some(name) = name_opt {
                println!("name: {name}");
                if name == DEVICE_NAME && peripherals.len() >= 2 { // for testing return as soon as Watch and one other device is found.
                    tx.send(BthrInfo::DiscoveredPeripherals(peripherals.clone()));
                    return Ok(peripherals)
                }
            }
            
        }
        println!("\n");
    }
}

pub async fn bt_heartrate(tx: Sender<BthrInfo>) -> Result<()> {


    let mut peripherals = get_peripherals(tx.clone()).await?;

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
                            tx.send(BthrInfo::HeartRate { 
                                live_heart_rate: hr,
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