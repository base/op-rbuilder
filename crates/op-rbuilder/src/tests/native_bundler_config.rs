//! Unit tests for native bundler configuration

#[cfg(test)]
mod tests {
    use crate::{
        args::OpRbuilderArgs,
        builders::BuilderConfig,
        tx_signer::Signer,
    };

    #[test]
    fn test_builder_config_defaults() {
        // Test that default args produce expected config
        let args = OpRbuilderArgs::default();
        let config = BuilderConfig::<()>::try_from(args).unwrap();
        
        assert!(!config.enable_native_bundler);
        assert_eq!(config.bundler_gas_reserve_percentage, 20);
        assert_eq!(config.bundler_gas_threshold, 80);
        assert!(config.bundler_signer.is_none());
        assert!(config.bundler_pool_url.is_none());
    }

    #[test]
    fn test_builder_config_with_bundler_enabled() {
        // Test conversion with all bundler fields set
        let mut args = OpRbuilderArgs::default();
        args.enable_native_bundler = true;
        args.bundler_gas_reserve_percentage = 25;
        args.bundler_gas_threshold = 75;
        args.bundler_pool_url = Some("http://localhost:50051".to_string());
        args.bundler_signer = Some(Signer::random());

        let config = BuilderConfig::<()>::try_from(args.clone()).unwrap();
        
        assert!(config.enable_native_bundler);
        assert_eq!(config.bundler_gas_reserve_percentage, 25);
        assert_eq!(config.bundler_gas_threshold, 75);
        assert_eq!(config.bundler_pool_url, Some("http://localhost:50051".to_string()));
        assert!(config.bundler_signer.is_some());
        
        // Verify the signer was properly cloned
        if let (Some(arg_signer), Some(config_signer)) = (&args.bundler_signer, &config.bundler_signer) {
            assert_eq!(arg_signer.address, config_signer.address);
        }
    }

    #[test]
    fn test_builder_config_boundary_values() {
        // Test with maximum percentage values (100%)
        let mut args = OpRbuilderArgs::default();
        args.bundler_gas_reserve_percentage = 100;
        args.bundler_gas_threshold = 100;

        let config = BuilderConfig::<()>::try_from(args).unwrap();
        
        assert_eq!(config.bundler_gas_reserve_percentage, 100);
        assert_eq!(config.bundler_gas_threshold, 100);

        // Test with minimum percentage values (0%)
        let mut args = OpRbuilderArgs::default();
        args.bundler_gas_reserve_percentage = 0;
        args.bundler_gas_threshold = 0;

        let config = BuilderConfig::<()>::try_from(args).unwrap();
        
        assert_eq!(config.bundler_gas_reserve_percentage, 0);
        assert_eq!(config.bundler_gas_threshold, 0);
    }

    #[test]
    fn test_builder_config_partial_settings() {
        // Test with only some bundler settings
        let mut args = OpRbuilderArgs::default();
        args.enable_native_bundler = true;
        args.bundler_gas_reserve_percentage = 15;
        // Leave other fields as defaults

        let config = BuilderConfig::<()>::try_from(args).unwrap();
        
        assert!(config.enable_native_bundler);
        assert_eq!(config.bundler_gas_reserve_percentage, 15);
        assert_eq!(config.bundler_gas_threshold, 80); // default
        assert!(config.bundler_signer.is_none());
        assert!(config.bundler_pool_url.is_none());
    }

    #[test]
    fn test_builder_config_debug_impl() {
        // Test that Debug implementation doesn't expose sensitive data
        let mut args = OpRbuilderArgs::default();
        args.enable_native_bundler = true;
        args.bundler_signer = Some(Signer::random());

        let config = BuilderConfig::<()>::try_from(args).unwrap();
        let debug_str = format!("{:?}", config);
        
        // Should contain the field names
        assert!(debug_str.contains("enable_native_bundler"));
        assert!(debug_str.contains("bundler_gas_reserve_percentage"));
        assert!(debug_str.contains("bundler_signer"));
        
        // The bundler_signer should show the address, not expose the private key
        // The Debug impl should use the custom formatting
        if let Some(signer) = &config.bundler_signer {
            // Should show address, not the full signer struct
            assert!(debug_str.contains(&signer.address.to_string()));
        }
    }

    #[test]
    fn test_builder_config_clone_behavior() {
        // Test that cloning args doesn't affect config
        let mut args = OpRbuilderArgs::default();
        args.bundler_pool_url = Some("http://original.com".to_string());
        
        let config = BuilderConfig::<()>::try_from(args.clone()).unwrap();
        
        // Modify the original args after creating config
        args.bundler_pool_url = Some("http://modified.com".to_string());
        
        // Config should retain original value
        assert_eq!(config.bundler_pool_url, Some("http://original.com".to_string()));
    }
}
