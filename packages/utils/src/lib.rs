use std::time::Duration;

use tokio::time::sleep;

pub async fn read_file_util<O: From<Vec<u8>>>(path: &str) -> O {
    loop {
        match tokio::fs::read(path).await {
            Ok(buf) => {
                break O::from(buf);
            }
            Err(e) => {
                log::error!("Read tunnel cert error: {:?}", e);
                sleep(Duration::from_secs(30)).await;
            }
        }
    }
}
