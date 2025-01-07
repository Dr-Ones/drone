# 🚁 Dr. Ones Drone

Drone node implementation for the Advanced Programming class at UniTn in 2024/2025.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
dr_ones = { git = "https://github.com/Dr-Ones/drone" }
```

## Usage

### Logging Control

You can enable or disable logging output globally:

```rust
// Disable all node logging (useful for tests or high-load scenarios)
dr_ones::disable_logging();

// Re-enable logging when needed
dr_ones::enable_logging();
```

## Testing

```bash
cargo test
```

## Support

Need help? Join our Telegram channel: [Dr_Ones_Customer_Support](https://t.me/Dr_Ones_Customer_Support)