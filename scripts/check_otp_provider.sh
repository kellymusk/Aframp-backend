#!/usr/bin/env bash
# #1241 — pre-deploy guard: refuse OTP_PROVIDER=mock in production-like environments.
#
# MockOtpProvider (src/otp/mock.rs) logs the full OTP message, including the
# 6-digit code, via tracing::info!. That is fine on a laptop but leaks live
# codes to the log aggregator if it is accidentally enabled in staging/prod.
#
# Usage: APP_ENV=production scripts/check_otp_provider.sh [path/to/.env]
# Exits 1 when OTP_PROVIDER=mock and APP_ENV (or ENVIRONMENT) is not dev/local/test.
set -euo pipefail

env_file="${1:-}"
if [[ -n "$env_file" && -f "$env_file" ]]; then
  # shellcheck disable=SC1090
  set -a; source "$env_file"; set +a
fi

provider="${OTP_PROVIDER:-termii}"
environment="${APP_ENV:-${ENVIRONMENT:-development}}"

case "${environment,,}" in
  dev|development|local|test|testing|ci) safe_env=1 ;;
  *) safe_env=0 ;;
esac

if [[ "${provider,,}" == "mock" ]]; then
  if [[ "$safe_env" == 0 ]]; then
    echo "ERROR: OTP_PROVIDER=mock in '$environment' — OTP codes would be logged in plaintext." >&2
    echo "       Set OTP_PROVIDER=termii (with TERMII_API_KEY) for production-like environments." >&2
    exit 1
  fi
  echo "WARNING: OTP_PROVIDER=mock — OTP codes are logged in plaintext. Never use real phone numbers." >&2
fi
echo "OK: OTP_PROVIDER=$provider, environment=$environment"
