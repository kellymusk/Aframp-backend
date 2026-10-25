# #1241 — Stop logging plaintext OTP codes in `MockOtpProvider`

**Status:** partially done in this PR; `src/` change proposed for a maintainer.

## Problem

`src/otp/mock.rs` runs `tracing::info!(phone, message, ...)`, where `message`
contains the full 6-digit code. If `OTP_PROVIDER=mock` leaks into staging or
production, live codes land in the log aggregator.

## Done in this PR (contributor-allowed paths)

- `.env.example` — explicit warning that `mock` must never be used with real
  phone numbers or in production-like environments.
- `scripts/check_otp_provider.sh` — pre-deploy guard: exits non-zero when
  `OTP_PROVIDER=mock` and `APP_ENV`/`ENVIRONMENT` is not dev/local/test.

## Proposed `src/` changes (maintainer)

1. Mask the code in the log line. Tests keep reading the full message from the
   in-memory `SENT` map via `last_message_for`, so nothing depends on the log:

```rust
fn mask_digits(message: &str) -> String {
    let mut seen = 0;
    message
        .chars()
        .map(|c| if c.is_ascii_digit() { seen += 1; if seen > 2 { '*' } else { c } } else { c })
        .collect()
}

tracing::info!(phone, message = %mask_digits(message), "mock OTP send (no real SMS delivered)");
```

2. Startup warning in `Config::from_env` when the mock provider is selected:

```rust
if otp_provider == OtpProviderKind::Mock {
    tracing::warn!("OTP_PROVIDER=mock — OTP codes are not delivered; never use in production");
}
```

   Optionally hard-fail when an `APP_ENV=production` variable is introduced.
