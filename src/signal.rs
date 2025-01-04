use btleplug::platform::Peripheral;



pub enum BthrSignal {
    HeartRate {
        heart_rate: u8,
    },
    DiscoveredPeripherals(Vec<String>),
    ScanStarted,
    ScanStopped,
    ActiveDevice(String),
    DeviceDisconnected, // TODO maybe add message why it disconnected and reuse this variant
}

pub enum GuiSignal {
    StartScanning,
    StopScanning,
    ConnectDevice(String),
}

pub enum TaskSignal {
    PeripheralsFound(Vec<Peripheral>),
    NotificationStreamAcquired,
    HeartRatePing,
    PeripheralNotFound(String),
    ConnectionFailed,
    DiscoveringServicesFailed,
    HrCharNotFound,
    CharSubscriptionFailed,
    NotificationStreamFailed,
    PeripheralDisconnected,
    AdapterNotFound,
    FailedScan,
}