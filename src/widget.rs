use eframe::egui::{Button, Color32, Label, RichText, Rounding};


pub fn get_heart_rate_label(heart_rate: u8) -> Label {
    let live_hr_text = RichText::new(format!("HR: {}", heart_rate))
        .color(Color32::RED)
        .background_color(Color32::WHITE)
        .size(40.0);

    let hr_label = Label::new(live_hr_text);

    hr_label
}

pub fn get_device_button(device_name: &String) -> Button {
    let device_text = RichText::new(device_name)
        .color(Color32::WHITE)
        .size(20.0);
    
    let device_button = Button::new(device_text)
        .fill(Color32::BLUE)
        .rounding(Rounding::same(8.0))
        .selected(false);

    device_button
}