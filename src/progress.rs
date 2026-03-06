use indicatif::{ProgressBar, ProgressStyle, MultiProgress};
use std::sync::Arc;

pub struct ProgressManager {
    multi: Arc<MultiProgress>,
}

impl ProgressManager {
    pub fn new() -> Self {
        Self {
            multi: Arc::new(MultiProgress::new()),
        }
    }

    pub fn create_transfer_progress(&self, file_name: &str, total_bytes: u64) -> ProgressBar {
        let pb = self.multi.add(ProgressBar::new(total_bytes));

        pb.set_style(
            ProgressStyle::default_bar()
                .template("{msg} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec})")
                .unwrap()
                .progress_chars("=>-")
        );
        pb.set_message(format!("{}", truncate_filename(file_name, 40)));
        pb
    }

    pub fn create_receive_progress(&self, file_name: &str, total_bytes: u64) -> ProgressBar {
        let pb = self.multi.add(ProgressBar::new(total_bytes));

        pb.set_style(
            ProgressStyle::default_bar()
                .template("{msg} [{bar:40.green/blue}] {bytes}/{total_bytes} ({bytes_per_sec})")
                .unwrap()
                .progress_chars("=>-")
        );
        pb.set_message(format!("{}", truncate_filename(file_name, 40)));
        pb
    }

    pub fn create_spinner(&self, message: &str) -> ProgressBar {
        let pb = self.multi.add(ProgressBar::new_spinner());

        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap()
        );

        pb.set_message(message.to_string());
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        pb
    }
}

fn truncate_filename(name: &str, max_len: usize) -> String {
    if name.len() <= max_len {
        name.to_string()
    } else {
        let half = (max_len - 3) / 2;
        format!("{}...{}", &name[..half], &name[name.len()-half..])
    }
}