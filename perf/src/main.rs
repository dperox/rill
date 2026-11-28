use goose::{
    client::GooseClientConfig,
    config::GooseConfiguration,
    goose::{GooseResponse, Transaction, TransactionError},
    metrics::GooseMetrics,
    prelude::*,
    test_plan::TestPlanStepAction,
};
use reqwest::{Client, StatusCode, Version, redirect::Policy};
use std::{
    env,
    error::Error,
    fs::File,
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const CASES: &str = "static-4k, proxy-4k, static-256k, proxy-256k";

static BODY_4K: [u8; 4096] = fixture();
static BODY_256K: [u8; 262144] = fixture();
// Goose may reset its metrics after ramp-up. A failed response still fails the run.
static FAILED: AtomicBool = AtomicBool::new(false);

const fn fixture<const SIZE: usize>() -> [u8; SIZE] {
    let pattern = b"0123456789abcdef";
    let mut body = [0; SIZE];
    let mut index = 0;
    while index < SIZE {
        body[index] = pattern[index % pattern.len()];
        index += 1;
    }
    body
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let configuration = GooseConfiguration::parse_args_default_or_exit();
    let attack = GooseAttack::initialize_with_config(configuration.clone())?;
    let case = env::var("JAPKA_PERF_CASE")
        .map_err(|_| format!("set JAPKA_PERF_CASE to one of {CASES}"))?;
    let (path, expected): (&str, &[u8]) = match case.as_str() {
        "static-4k" => ("/4096.bin", &BODY_4K),
        "proxy-4k" => ("/api/4096.bin?bench=1", &BODY_4K),
        "static-256k" => ("/262144.bin", &BODY_256K),
        "proxy-256k" => ("/api/262144.bin?bench=1", &BODY_256K),
        _ => {
            return Err(
                format!("invalid JAPKA_PERF_CASE {case:?}; expected one of {CASES}").into(),
            );
        }
    };

    let request = Transaction::new(Arc::new(move |user| {
        Box::pin(async move {
            let result = check_response(user, path, expected).await;
            if result
                .as_ref()
                .is_err_and(|error| !matches!(**error, TransactionError::RequestCanceled { .. }))
            {
                FAILED.store(true, Ordering::Relaxed);
            }
            result
        })
    }))
    .set_name(&case);
    let metrics = attack
        .set_default(GooseDefault::NoTelnet, true)?
        .register_scenario(
            Scenario::new(&case)
                .register_transaction(
                    transaction!(configure_client)
                        .set_name("client")
                        .set_on_start(),
                )
                .register_transaction(request),
        )
        .execute()
        .await?;

    if FAILED.load(Ordering::Relaxed) {
        return Err(format!("{case}: at least one response failed validation").into());
    }
    let transaction = metrics
        .transactions
        .iter()
        .flatten()
        .find(|transaction| transaction.transaction_name.name_for_transaction() == case)
        .ok_or("benchmark requires transaction metrics")?;
    let validated_requests = transaction.success_count;
    if validated_requests == 0 {
        return Err(format!("{case}: no successful responses recorded").into());
    }
    let p95_us = transaction.run_times.percentile_us(0.95);
    let elapsed_us = measurement_us(&configuration, &metrics, validated_requests)?;
    let requests_per_second = validated_requests as f64 * 1_000_000.0 / elapsed_us as f64;
    if let Some(path) = env::var_os("JAPKA_PERF_RESULT") {
        let path = PathBuf::from(path);
        let result = serde_json::json!({
            "case": case,
            "validated_requests": validated_requests,
            "requests_per_second": requests_per_second,
            "p95_us": p95_us,
        });
        File::create_new(&path)
            .and_then(|mut file| file.write_all(result.to_string().as_bytes()))
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    }
    println!(
        "{case}: {validated_requests} validated responses, {requests_per_second:.2} req/s, transaction p95 {p95_us} µs",
    );
    Ok(())
}

fn measurement_us(
    config: &GooseConfiguration,
    metrics: &GooseMetrics,
    validated_requests: usize,
) -> Result<i64, Box<dyn Error>> {
    use TestPlanStepAction::{Canceling, Decreasing, Finished, Maintaining};

    let (finished, phases) = metrics
        .history
        .split_last()
        .filter(|(last, _)| matches!(last.action, Finished))
        .ok_or("benchmark did not finish")?;
    let started = if let Some(iterations) = config.iterations.filter(|&count| count > 0) {
        let users = config
            .users
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |count| count.get()));
        if users.checked_mul(iterations) != Some(validated_requests) {
            return Err("benchmark ended before all users completed their iterations".into());
        }
        phases.first()
    } else {
        if phases.iter().any(|phase| matches!(phase.action, Canceling))
            || !phases
                .last()
                .is_some_and(|phase| matches!(phase.action, Decreasing))
        {
            return Err("timed benchmark ended before its scheduled stop".into());
        }
        if config.run_time.is_none() {
            return Err("timed benchmark requires --run-time".into());
        }
        if config.no_reset_metrics {
            phases.first()
        } else {
            // Goose resets metrics as it enters the hold. Finished follows the drain.
            phases
                .iter()
                .find(|phase| matches!(phase.action, Maintaining))
        }
    }
    .ok_or("benchmark did not reach its measurement phase")?;

    finished
        .timestamp
        .signed_duration_since(started.timestamp)
        .num_microseconds()
        .filter(|&elapsed| elapsed > 0)
        .ok_or_else(|| "benchmark measurement duration must be positive".into())
}

async fn configure_client(user: &mut GooseUser) -> TransactionResult {
    let mut client = Client::builder()
        .no_proxy()
        .no_gzip()
        .http1_only()
        .redirect(Policy::none());
    if let Some(timeout) = GooseClientConfig::from(&user.config).timeout {
        client = client.timeout(timeout);
    }

    let result = user.set_client_builder(client).await;
    if result.is_err() {
        FAILED.store(true, Ordering::Relaxed);
    }
    result.map_err(Into::into)
}

async fn check_response(user: &mut GooseUser, path: &str, expected: &[u8]) -> TransactionResult {
    let GooseResponse {
        mut request,
        response,
    } = user.get(path).await?;
    let response = response?; // Goose has already recorded any send error.
    let status = response.status();
    let version = response.version();
    let body = response.bytes().await;

    if status != StatusCode::OK || version != Version::HTTP_11 {
        return user.set_failure(
            &format!("expected HTTP/1.1 200, received {version:?} {status}"),
            &mut request,
            None,
            None,
        );
    }
    match body {
        Ok(body) if body.as_ref() == expected => Ok(()),
        Ok(_) => user.set_failure(
            "response body differs from fixture",
            &mut request,
            None,
            None,
        ),
        Err(_) => user.set_failure("could not read response body", &mut request, None, None),
    }
}
