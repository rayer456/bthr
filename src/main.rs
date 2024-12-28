use std::{sync::mpsc::{channel, Sender as StdSender}, thread::sleep, time::{Duration, SystemTime}};

use bthr::BthrManager;
use btleplug::platform::Peripheral;
use btleplug::api::Peripheral as _;
use eframe::{egui::{self, Button, Color32, Label, RichText, Rounding}, glow::INFO_LOG_LENGTH};
use tokio::{spawn, sync::mpsc::{self, Receiver as TokioReceiver}};

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
        let _ = bthr_manager.run().await;
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
}

impl MyApp {
    fn new(_cc: &eframe::CreationContext<'_>, rx_from_bthr: TokioReceiver<BthrSignal>, tx_from_gui: StdSender<GuiSignal>) -> Self {
        MyApp {
            rx_from_bthr,
            live_heart_rate: 0,
            frame_time: Duration::from_secs_f64(1.0/MAX_FPS),
            peris: vec![],
            tx_from_gui,
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
            BthrSignal::StartScan => println!("Started scan..."),
        }
    }

    fn update_live_heart_rate(&mut self, heart_rate: u8) {
        // Can probably remove this method
        self.live_heart_rate = heart_rate;
    }


}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        
        let now = SystemTime::now(); // FPS related

        // Read bt channel
        self.read_channel();

        // GUI
        let live_hr_label = widget::get_heart_rate_label(self.live_heart_rate);
        
        let central_panel = egui::CentralPanel::default();
        central_panel.show(ctx, |ui| {

            // hr label
            ui.add(live_hr_label);

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