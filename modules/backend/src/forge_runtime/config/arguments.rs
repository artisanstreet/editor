//! Exact option dispatch for local and explicitly networked Forge launches.
use super::{
    ADMISSION_CAPACITY_OPTION, ADMISSION_TIMEOUT_OPTION, BOOTSTRAP_OPTION, CERTIFICATE_OPTION,
    CUSTODY_OPTION, DATABASE_OPTION, DRAIN_TIMEOUT_OPTION, ForgeConfigError,
    HANDSHAKE_TIMEOUT_OPTION, NATIVE_CLAIM_LEASE_OPTION, NATIVE_LAUNCH_DEADLINE_OPTION,
    NATIVE_MAX_COMMAND_RETRIES_OPTION, NATIVE_POLL_INTERVAL_OPTION, NATIVE_PROMPT_DELIVERY_OPTION,
    NATIVE_QUEUE_CAPACITY_OPTION, NATIVE_RETRY_BACKOFF_OPTION, NATIVE_SHUTDOWN_BUDGET_OPTION,
    NATIVE_STREAM_AFTER_OPTION, OsStr, OsString, PRIVATE_KEY_OPTION, ParsedForgeArguments,
    READY_FILE_OPTION, REQUEST_TIMEOUT_OPTION, REQUESTS_PER_CONNECTION_OPTION, explicit_path,
    parse_native_option, set_capacity, set_duration, set_path,
};

pub(super) fn recognized_option(option: &OsStr) -> Option<&'static str> {
    match option.to_str()? {
        "--listen" => Some("--listen"),
        DATABASE_OPTION => Some(DATABASE_OPTION),
        CUSTODY_OPTION => Some(CUSTODY_OPTION),
        CERTIFICATE_OPTION => Some(CERTIFICATE_OPTION),
        PRIVATE_KEY_OPTION => Some(PRIVATE_KEY_OPTION),
        BOOTSTRAP_OPTION => Some(BOOTSTRAP_OPTION),
        READY_FILE_OPTION => Some(READY_FILE_OPTION),
        ADMISSION_TIMEOUT_OPTION => Some(ADMISSION_TIMEOUT_OPTION),
        HANDSHAKE_TIMEOUT_OPTION => Some(HANDSHAKE_TIMEOUT_OPTION),
        REQUEST_TIMEOUT_OPTION => Some(REQUEST_TIMEOUT_OPTION),
        DRAIN_TIMEOUT_OPTION => Some(DRAIN_TIMEOUT_OPTION),
        ADMISSION_CAPACITY_OPTION => Some(ADMISSION_CAPACITY_OPTION),
        REQUESTS_PER_CONNECTION_OPTION => Some(REQUESTS_PER_CONNECTION_OPTION),
        NATIVE_CLAIM_LEASE_OPTION => Some(NATIVE_CLAIM_LEASE_OPTION),
        NATIVE_LAUNCH_DEADLINE_OPTION => Some(NATIVE_LAUNCH_DEADLINE_OPTION),
        NATIVE_POLL_INTERVAL_OPTION => Some(NATIVE_POLL_INTERVAL_OPTION),
        NATIVE_RETRY_BACKOFF_OPTION => Some(NATIVE_RETRY_BACKOFF_OPTION),
        NATIVE_SHUTDOWN_BUDGET_OPTION => Some(NATIVE_SHUTDOWN_BUDGET_OPTION),
        NATIVE_QUEUE_CAPACITY_OPTION => Some(NATIVE_QUEUE_CAPACITY_OPTION),
        NATIVE_MAX_COMMAND_RETRIES_OPTION => Some(NATIVE_MAX_COMMAND_RETRIES_OPTION),
        NATIVE_PROMPT_DELIVERY_OPTION => Some(NATIVE_PROMPT_DELIVERY_OPTION),
        NATIVE_STREAM_AFTER_OPTION => Some(NATIVE_STREAM_AFTER_OPTION),
        _ => None,
    }
}

pub(super) fn parse_option(
    parsed: &mut ParsedForgeArguments,
    option: &'static str,
    raw_value: OsString,
) -> Result<(), ForgeConfigError> {
    if option.starts_with("--native-run-") {
        return parse_native_option(&mut parsed.native_run, option, raw_value);
    }
    match option {
        "--listen" => {
            if parsed.listen.is_some() {
                return Err(ForgeConfigError::Duplicate { option: "--listen" });
            }
            let address: std::net::SocketAddr = raw_value
                .to_str()
                .and_then(|s| s.parse().ok())
                .ok_or(ForgeConfigError::InvalidListen)?;
            if address.ip().is_multicast()
                || matches!(address.ip(), std::net::IpAddr::V4(ip) if ip.is_broadcast())
            {
                return Err(ForgeConfigError::InvalidListen);
            }
            parsed.listen = Some(address);
            Ok(())
        }
        DATABASE_OPTION => set_path(&mut parsed.database, option, raw_value),
        CUSTODY_OPTION => set_path(&mut parsed.custody, option, raw_value),
        CERTIFICATE_OPTION => {
            parsed
                .certificate_der
                .push(explicit_path(option, raw_value.into())?);
            Ok(())
        }
        PRIVATE_KEY_OPTION => set_path(&mut parsed.private_key_der, option, raw_value),
        BOOTSTRAP_OPTION => set_path(&mut parsed.bootstrap_capability, option, raw_value),
        READY_FILE_OPTION => set_path(&mut parsed.ready_file, option, raw_value),
        ADMISSION_TIMEOUT_OPTION => set_duration(
            &mut parsed.admission_timeout_ms,
            option,
            raw_value.as_os_str(),
        ),
        HANDSHAKE_TIMEOUT_OPTION => set_duration(
            &mut parsed.handshake_timeout_ms,
            option,
            raw_value.as_os_str(),
        ),
        REQUEST_TIMEOUT_OPTION => set_duration(
            &mut parsed.request_timeout_ms,
            option,
            raw_value.as_os_str(),
        ),
        DRAIN_TIMEOUT_OPTION => {
            set_duration(&mut parsed.drain_timeout_ms, option, raw_value.as_os_str())
        }
        ADMISSION_CAPACITY_OPTION => set_capacity(
            &mut parsed.admission_capacity,
            option,
            raw_value.as_os_str(),
        ),
        REQUESTS_PER_CONNECTION_OPTION => set_capacity(
            &mut parsed.requests_per_connection,
            option,
            raw_value.as_os_str(),
        ),
        _ => Err(ForgeConfigError::UnknownOption),
    }
}
