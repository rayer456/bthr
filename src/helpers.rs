use btleplug::{api::Peripheral, platform::Peripheral as PlatformPeripheral};

pub async fn find_peri_by_name(sought_peri: &String, peris: &Vec<PlatformPeripheral>) -> Option<PlatformPeripheral> {
    for peri in peris {
        let Some(peri_name) = get_peripheral_name(peri).await else { continue; };
        if *sought_peri == peri_name {
            println!("Found peri by name");
            return Some(peri.clone());
        }
    }
    println!("Didn't find peri by name");
    None
}

pub async fn get_peripheral_name(peripheral: &PlatformPeripheral) -> Option<String> {
    let Ok(Some(properties)) = peripheral.properties().await else { return None; };

    properties.local_name
}