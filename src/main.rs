use std::{thread::sleep, time::{Duration, Instant, SystemTime}};

use eframe::{egui::{self, Color32, Key, Label, Modifiers, Rect, RichText}, Frame};
use tokio::{spawn, sync::{mpsc::{self, Receiver, Sender}, oneshot}};

use futures::StreamExt;
use uuid::Uuid;

use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::{Adapter, Manager};
use anyhow::{bail, Result};


const MAX_FPS: f64 = 165.0;

const DEVICE_NAME: &'static str = "COROS PACE Pro B69E81";
const HEART_RATE_MEASUREMENT_UUID: Uuid = Uuid::from_u128(0x00002a3700001000800000805f9b34fb);

#[tokio::main]
async fn main() {
    let (tx, rx) = mpsc::channel(128);
    let _ = spawn(bt_heartrate(tx));


    let native_options = eframe::NativeOptions::default();
    let _ = eframe::run_native(
        "My app", 
        native_options, 
        Box::new(|cc| Ok(Box::new(MyApp::new(cc, rx)))),
    );
}

async fn bt_heartrate(tx: Sender<u8>) -> Result<()> {

    let manager = Manager::new().await?;
    let adapter_list = manager.adapters().await?;
    if adapter_list.is_empty() {
        eprintln!("No Bluetooth adapters found");
    }

    for adapter in adapter_list.iter() {
        println!("Starting scan...");
        adapter
            .start_scan(ScanFilter::default())
            .await
            .expect("Can't scan BLE adapter for connected devices...");
        tokio::time::sleep(Duration::from_secs(4)).await;
        let peripherals = adapter.peripherals().await?;

        if peripherals.is_empty() {
            eprintln!("->>> BLE peripheral devices were not found, sorry. Exiting...");
        } else {
            // All peripheral devices in range.
            for peripheral in peripherals.iter() {
                let properties = peripheral.properties().await?;
                let is_connected = peripheral.is_connected().await?;
                let local_name = properties
                    .unwrap()
                    .local_name
                    .unwrap_or(String::from("(peripheral name unknown)"));
                println!(
                    "Peripheral {:?} is connected: {:?}",
                    &local_name, is_connected
                );
                // Check if it's the peripheral we want.
                if local_name.contains(DEVICE_NAME) {
                    println!("Found matching peripheral {:?}...", &local_name);
                    if !is_connected {
                        // Connect if we aren't already connected.
                        if let Err(err) = peripheral.connect().await {
                            eprintln!("Error connecting to peripheral, skipping: {}", err);
                            continue;
                        }
                    }
                    let is_connected = peripheral.is_connected().await?;
                    println!(
                        "Now connected ({:?}) to peripheral {:?}.",
                        is_connected, &local_name
                    );
                    if is_connected {
                        println!("Discover peripheral {:?} services...", local_name);
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
                                    tx.send(hr).await;
                                }
                            }
                        }
                        println!("Disconnecting from peripheral {:?}...", local_name);
                        peripheral.disconnect().await?;
                    }
                } else {
                    println!("Not watch, skipping...");
                }
            }
        }
    }
    Ok(())
}

struct MyApp {
    btrx: Receiver<u8>,
    data: u8,
    frame_time: Duration,
}

impl MyApp {
    fn new(_cc: &eframe::CreationContext<'_>, btrx: Receiver<u8>) -> Self {
        MyApp {
            btrx,
            data: 0,
            frame_time: Duration::from_secs_f64(1.0/MAX_FPS),
        }
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ctx.request_repaint_after(Duration::from_millis(150));


        let now = SystemTime::now();

        let range = egui::Rangef::new(200.0, 250.0);
        let frame = egui::Frame::none().fill(Color32::LIGHT_BLUE);

        let top_panel = egui::TopBottomPanel::top("top_panel")
            .height_range(range)
            .frame(frame);

        top_panel.show(ctx, |ui| {
            let r_side_panel = egui::SidePanel::right("right_side_panel")
                .min_width(50.0)
                .max_width(120.0);

            r_side_panel.show(ctx, |ui| {
                ui.label("some text on the top right panel");
            });

            ui.label("Some text on the left side of the top panel");
        });

        let bottom_panel = egui::TopBottomPanel::bottom("bottom_panel")
            .height_range(range);

        /* if ctx.input(|i| i.modifiers.contains(Modifiers::SHIFT) && i.key_down(Key::B)) {
            self.text_val = "B is being pressed while holding shift".to_string();
        } else {
            self.text_val = "default text".to_string();
        } */


        self.data = self.btrx.try_recv().unwrap_or(self.data);


        let my_text = RichText::new(self.data.to_string()).color(Color32::RED).background_color(Color32::WHITE);
        let my_label = Label::new(my_text).halign(egui::Align::Center);
        bottom_panel.show(ctx, |ui| {
            
            ui.add(my_label);

            ui.horizontal(|ui| {
                ui.label("test hor 1");
                ui.label("test hor 2");
            });


            let collapsing_header = egui::CollapsingHeader::new("Collapsing header").default_open(true);
            collapsing_header.show(ui, |ui2| {
                ui2.label("text in collapsing header");
            });


        });

        let elapsed = now.elapsed().unwrap();

        ctx.request_repaint();
        sleep(self.frame_time - elapsed);

        
    }
}