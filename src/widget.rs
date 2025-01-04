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

pub fn get_active_device_frame(device_name: &String) -> Label {
    let device_text = RichText::new(device_name)
        .color(Color32::WHITE)
        .background_color(Color32::RED)
        .size(20.0);
    
    let active_device_label = Label::new(device_text);

    active_device_label
}

pub fn get_disconnect_device_button() -> Button<'static> {
    let text = RichText::new("DC")
        .color(Color32::RED)
        .background_color(Color32::WHITE)
        .size(20.0);

    let disconnect_button = Button::new(text)
        .fill(Color32::BLUE)
        .rounding(Rounding::same(8.0));

    disconnect_button
}