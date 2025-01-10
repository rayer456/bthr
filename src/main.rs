use std::{sync::mpsc::{Sender as StdSender}, thread::sleep, time::{Duration, SystemTime}};

use bthr::BthrManager;
use eframe::egui::{self, Button, Frame};
use tokio::{spawn, sync::mpsc::{Receiver as TokioReceiver}};

mod bthr;
mod fake;
mod signal;
mod widget;

use signal::{BthrSignal, GuiSignal};

const MAX_FPS: f64 = 165.0;


#[tokio::main]
async fn main() {
    let (tx, rx) = tokio::sync::mpsc::channel(128);
    let (tx_from_gui, rx_to_gui) = std::sync::mpsc::channel();

    /* let (t1, r1) = channel();

    spawn(async move {
        loop {
            let Ok(data) = r1.try_recv() else { continue; };
            println!("data in thread: {}", data);
        }
    });

    let mut counter = 0;
    loop {
        t1.send(counter);

        counter += 1;
        sleep(Duration::from_secs(1));
    } */
    

    // spawn(fake::transmit_fake_hr_data(tx));
    let mut bthr_manager = BthrManager::new(tx, rx_to_gui);
    spawn(async move {
        let _ = bthr_manager.main_loop().await;
    });


    let native_options = eframe::NativeOptions::default();
    let _ = eframe::run_native(
        "bthr", 
        native_options, 
        Box::new(|cc| Ok(Box::new(MyApp::new(cc, rx, tx_from_gui)))),
    );
}


struct MyApp {
    rx_from_bthr: TokioReceiver<BthrSignal>,
    live_heart_rate: u8,
    frame_time: Duration,
    peris: Vec<String>,
    tx_from_gui: StdSender<GuiSignal>,
    is_scanning: bool,
    active_device: Option<String>,
    busy_connecting: bool,
}

impl MyApp {
    fn new(_cc: &eframe::CreationContext<'_>, rx_from_bthr: TokioReceiver<BthrSignal>, tx_from_gui: StdSender<GuiSignal>) -> Self {
        MyApp {
            rx_from_bthr,
            live_heart_rate: 0,
            frame_time: Duration::from_secs_f64(1.0/MAX_FPS),
            peris: vec![],
            tx_from_gui,
            is_scanning: false,
            active_device: None,
            busy_connecting: false,
        }
    }
}

impl MyApp {
    fn read_channel(&mut self) {
        // Check the type of the enum then does the appropriate thing
        let Ok(info) = self.rx_from_bthr.try_recv() else { return; };
        match info {
            BthrSignal::DiscoveredPeripherals(peris) => self.peris = peris,
            BthrSignal::HeartRate { heart_rate } => self.update_live_heart_rate(heart_rate),
            BthrSignal::ScanStarted => self.update_is_scanning(true),
            BthrSignal::ScanStopped => self.update_is_scanning(false),
            BthrSignal::ActiveDevice(device_name) => self.set_active_device(device_name),
            BthrSignal::DeviceDisconnected => self.device_disconnect(), // Add a reason for disconnect
            BthrSignal::Connecting => self.busy_connecting(),
        }
    }

    fn busy_connecting(&mut self) {
        self.busy_connecting = true;
    }

    fn set_active_device(&mut self, device_name: String) {
        self.active_device = Some(device_name);
        self.busy_connecting = false; // Active device implicitly means process of connecting is stopped.
    }

    fn device_disconnect(&mut self) {
        self.active_device = None;
        self.live_heart_rate = 0;
    }

    fn update_is_scanning(&mut self, is_scanning: bool) {
        self.is_scanning = is_scanning;

        if !is_scanning {
            self.peris.clear();
        }
    }

    fn update_live_heart_rate(&mut self, heart_rate: u8) {
        self.live_heart_rate = heart_rate;
    }

    fn get_scanning_text(&self) -> String {
        match self.is_scanning {
            true => "Stop Scanning".to_string(),
            false => "Start Scanning".to_string(),
        }
    }

    fn send_scanning_signal(&self) {
        let _ = match self.is_scanning {
            false => self.tx_from_gui.send(GuiSignal::StartScanning),
            true => self.tx_from_gui.send(GuiSignal::StopScanning),
        };
    }


}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        
        let now = SystemTime::now(); // FPS related

        // Read bt channel
        self.read_channel();

        
        let central_panel = egui::CentralPanel::default();
        central_panel.show(ctx, |ui| {

            
            ui.horizontal(|ui| {
                // hr label
                let live_hr_label = widget::get_heart_rate_label(self.live_heart_rate);
                ui.add(live_hr_label);

                // active device
                if let Some(ref active_device_str) = self.active_device {
                    let active_device_label = widget::get_active_device_frame(active_device_str);
                    Frame::none()
                        .fill(egui::Color32::RED)
                        .show(ui, |ui| {
                            ui.add(active_device_label);
                        });
                    let dc_button = widget::get_disconnect_device_button();
                    if ui.add(dc_button).clicked() {
                        let _ = self.tx_from_gui.send(GuiSignal::DisconnectDevice);
                    }
                }

                if self.busy_connecting {
                    let busy_connecting_label = widget::get_busy_connecting_label();
                    ui.add(busy_connecting_label);
                }
            });

            let scanning_text = self.get_scanning_text();
            if ui.button(scanning_text).clicked() {
                self.send_scanning_signal();
            }

            // devices
            for device in &self.peris {
                let device_button = widget::get_device_button(device);
                let device_button_clicked = ui.add(device_button).clicked();

                if device_button_clicked {
                    println!("You clicked: {}", device);
                    // try to connect
                    let _unused_res = self.tx_from_gui.send(GuiSignal::ConnectDevice(device.clone()));
                }

            }
            
        });


        // FPS related
        let elapsed = now.elapsed().unwrap_or(Duration::from_micros(300));
    
        // println!("{:?}", elapsed);
        ctx.request_repaint();
        if elapsed < self.frame_time {
            sleep(self.frame_time - elapsed);
        }
    }

}