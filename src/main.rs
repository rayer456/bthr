use std::{thread::sleep, time::{Duration, SystemTime}};

use btleplug::platform::Peripheral;
use btleplug::api::Peripheral as _;
use eframe::{egui::{self, Color32, Label, RichText}, glow::INFO_LOG_LENGTH};
use tokio::{spawn, sync::mpsc::{self, Receiver}};

mod bthr;
mod fake;
mod bthr_info;

use bthr_info::BthrSignal;

const MAX_FPS: f64 = 165.0;


#[tokio::main]
async fn main() {
    let (tx, rx) = mpsc::channel(128);
    

    // spawn(fake::transmit_fake_hr_data(tx));
    spawn(bthr::bt_heartrate(tx));

    


    let native_options = eframe::NativeOptions::default();
    let _ = eframe::run_native(
        "bthr", 
        native_options, 
        Box::new(|cc| Ok(Box::new(MyApp::new(cc, rx)))),
    );
}


struct MyApp {
    btrx: Receiver<BthrSignal>,
    live_heart_rate: u8,
    frame_time: Duration,
    peris: Vec<String>,
    
}

impl MyApp {
    fn new(_cc: &eframe::CreationContext<'_>, btrx: Receiver<BthrSignal>) -> Self {
        MyApp {
            btrx,
            live_heart_rate: 0,
            frame_time: Duration::from_secs_f64(1.0/MAX_FPS),
            peris: vec![],
        }
    }
}

impl MyApp {
    fn read_channel(&mut self) {
        // Check the type of the enum then does the appropriate thing
        let Ok(info) = self.btrx.try_recv() else { return; };
        match info {
            BthrSignal::DiscoveredPeripherals(peris) => self.enumerate_peris(peris),
            BthrSignal::HeartRate { heart_rate } => self.update_live_heart_rate(heart_rate),
            BthrSignal::StartScan => println!("Started scan..."),
        }
    }

    fn enumerate_peris(&mut self, peris: Vec<String>) {
        for per in peris.iter() {
            println!("from egui thread -> name: {per}");
        }
        self.peris = peris;
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
        let live_hr_text = RichText::new(format!("HR: {}", self.live_heart_rate.to_string()))
            .color(Color32::RED)
            .background_color(Color32::WHITE)
            .size(40.0);

        let live_hr_label = Label::new(live_hr_text);

        let devices = self.peris
            .iter()
            .map(|d_str| Label::new(d_str.as_str()))
            .collect::<Vec<Label>>();

        let central_panel = egui::CentralPanel::default();
        central_panel.show(ctx, |ui| {
            ui.add(live_hr_label);
            for device in devices {
                ui.add(device);
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