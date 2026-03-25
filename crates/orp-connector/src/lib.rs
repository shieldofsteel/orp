pub mod adapters;
pub mod traits;

pub use adapters::adsb::AdsbConnector;
pub use adapters::ais::AisConnector;
pub use adapters::csv_watcher::CsvWatcherConnector;
pub use adapters::http_poller::HttpPollerConnector;
pub use adapters::mqtt::MqttConnector;
pub use adapters::websocket_client::WebSocketClientConnector;
pub use traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
