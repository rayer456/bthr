use std::time::Duration;

use rand::Rng;
use tokio::sync::mpsc::Sender;

// Can probably be removed
pub async fn transmit_fake_hr_data(tx: Sender<u8>) {
    loop {
        let random = rand::thread_rng().gen_range(50..70);
        let _ = tx.send(random).await;
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
}