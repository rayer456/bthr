use std::time::{Duration, Instant};
use btleplug::platform::{Peripheral as PlatformPeripheral};

use crate::helpers;

pub struct Reconnector {
    pub last_connection_failure: Option<Instant>,
    pub should_reconnect: Option<String>,
    last_connection: Instant,
}

impl Reconnector {
    pub fn new() -> Self {
        Reconnector {
            last_connection_failure: None,
            should_reconnect: None,
            last_connection: Instant::now(),
        }
    }

    pub fn prepare_for_reconnection(&mut self, active_device_name: String) {
        // Determines if connecting task should be restarted if current restarting period
        // is below a certain time threshold.
        // This function will set the correct variables for another
        // function to restart the connecting task.

        let now = Instant::now();
        let last_failure = self.last_connection_failure.unwrap_or(now);
        if self.last_connection_failure.is_none() {
            self.last_connection_failure = Some(last_failure);
        }

        let failing_for = now.duration_since(last_failure);

        // TODO: make time threshold configurable
        if failing_for > Duration::from_secs(30) {
            self.last_connection_failure = None;
            println!("Reached reconnection threshold"); // Do something in GUI here too
            return;
        }

        println!("Connection reset for {:?}", failing_for);

        self.should_reconnect = Some(active_device_name);
    }

    pub async fn should_reconnect(&mut self, peris: &Vec<PlatformPeripheral>) -> bool {
        // maybe only check every second or so
        let elapsed_since_last_check = self.last_connection.elapsed();

        if elapsed_since_last_check < Duration::from_millis(1500) {
            return false;
        }

        let Some(ref device) = self.should_reconnect else { return false; };
        let device = device.clone();

        if let Some(_) = helpers::find_peri_by_name(&device, peris).await { 
            self.should_reconnect = None; // Avoid infinite loop
            return true;
        }

        false
    }
}