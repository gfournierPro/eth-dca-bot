// src/lib.rs
#[cfg(test)]
mod tests {
    use super::*;
    use tokio_test;

    #[tokio::test]
    async fn test_config_validation() {
        let mut config = Config::default();
        config.binance.api_key = "test_key".to_string();
        config.binance.secret_key = "test_secret".to_string();

        assert!(validate_config(&config).is_ok());
    }

    #[tokio::test]
    async fn test_invalid_config() {
        let config = Config::default(); // Empty API keys
        assert!(validate_config(&config).is_err());
    }
}
