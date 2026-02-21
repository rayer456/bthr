use std::{time::{Duration, SystemTime}};
use anyhow::{bail, Result};

pub struct Pulse {
    notifications_stream_acquired_at: Option<SystemTime>,
    last_pulse: Option<SystemTime>,
    
}

impl Pulse {
    pub fn new() -> Self {
        Pulse {
            notifications_stream_acquired_at: None,
            last_pulse: None,
        }
    }

    pub fn reset_last_pulse(&mut self) {
        self.last_pulse = None;
    }

    pub fn reset_notifications_stream_acquire_time(&mut self) {
        self.notifications_stream_acquired_at = None;
    }

    pub fn notif_stream_acquired(&mut self) {
        self.notifications_stream_acquired_at = Some(SystemTime::now())
    }

    pub fn pulse_received(&mut self) {
        self.last_pulse = Some(SystemTime::now());
    }

    fn elapsed_notification_time(&self) -> Result<Duration> {
        let Some(notification_time) = self.notifications_stream_acquired_at else { bail!("notif stream is None"); };
        let Ok(notification_time_elapsed) = notification_time.elapsed() else { bail!("elapsed() failed"); };

        Ok(notification_time_elapsed)
    }

    fn elapsed_last_pulse(&self) -> Result<Duration> {
        let Some(last_pulse_time) = self.last_pulse else { bail!("last pulse is None"); };
        let Ok(last_pulse_elapsed) = last_pulse_time.elapsed() else { bail!("elapsed() failed"); };

        Ok(last_pulse_elapsed)
    }

    pub fn is_stuck(&self) -> bool {
        // Check if a pulse is expected at this time or not.
        // If a ping is expected, but we don't receive one past a set timeout threshold, 
        // then we make the assumption that the connecting task is stuck waiting for BT data input. 

        let Ok(notification_time_elapsed) = self.elapsed_notification_time() else { return false; };
        if self.last_pulse.is_none() && notification_time_elapsed > Duration::from_secs(10) {
            println!("HR STUCK");
            return true;
        }

        let Ok(last_pulse_elapsed) = self.elapsed_last_pulse() else { return false; };
        if last_pulse_elapsed > Duration::from_secs(10) {
            println!("HR STUCK");
            return true;
        }

        false
    }
}