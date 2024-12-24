use std::{thread::sleep, time::{Duration, SystemTime}};

use eframe::{egui::{self, Color32, Key, Label, Modifiers, Rect, RichText}, Frame};
use tokio::{spawn, sync::mpsc::{self, Receiver}};

mod bthr;


const MAX_FPS: f64 = 165.0;


#[tokio::main]
async fn main() {
    let (tx, rx) = mpsc::channel(128);
    let _ = spawn(bthr::bt_heartrate(tx));


    let native_options = eframe::NativeOptions::default();
    let _ = eframe::run_native(
        "My app", 
        native_options, 
        Box::new(|cc| Ok(Box::new(MyApp::new(cc, rx)))),
    );
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