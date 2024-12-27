use std::{thread::sleep, time::{Duration, SystemTime}};

use eframe::{egui::{self, Color32, Label, RichText}};
use tokio::{spawn, sync::mpsc::{self, Receiver}};

mod bthr;
mod fake;

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
    btrx: Receiver<u8>,
    live_heart_rate: u8,
    frame_time: Duration,
}

impl MyApp {
    fn new(_cc: &eframe::CreationContext<'_>, btrx: Receiver<u8>) -> Self {
        MyApp {
            btrx,
            live_heart_rate: 0,
            frame_time: Duration::from_secs_f64(1.0/MAX_FPS),
        }
    }
}

impl MyApp {
    fn update_live_heart_rate(&mut self) {
        self.live_heart_rate = self.btrx.try_recv().unwrap_or(self.live_heart_rate);
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        
        let now = SystemTime::now(); // FPS related

        // Update HR live_heart_rate
        self.update_live_heart_rate();


        // GUI
        let live_hr_text = RichText::new(format!("HR: {}", self.live_heart_rate.to_string()))
            .color(Color32::RED)
            .background_color(Color32::WHITE)
            .size(40.0);

        let live_hr_label = Label::new(live_hr_text);

        let central_panel = egui::CentralPanel::default();
        central_panel.show(ctx, |ui| {
            ui.add(live_hr_label);


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