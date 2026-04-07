use crate::models::OwnedMessage;

pub struct FetchResult {
    pub messages: Vec<OwnedMessage>,
    pub method: &'static str,
}

pub struct SearchProgress {
    pub total_results: u64,
    pub collected_results: u64,
}
