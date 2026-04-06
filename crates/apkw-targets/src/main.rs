mod adb;
mod cuttlefish;
mod ids;
mod jobs;
mod service;
mod state;

use apkw_proto::apkw::v1::target_service_server::TargetServiceServer;
use apkw_util::serve_grpc_with_telemetry;
use service::Svc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    apkw_util::promote_legacy_env();
    serve_grpc_with_telemetry(
        "apkw-targets",
        env!("CARGO_PKG_VERSION"),
        "targets",
        "APKW_TARGETS_ADDR",
        apkw_util::DEFAULT_TARGETS_ADDR,
        |server| server.add_service(TargetServiceServer::new(Svc::default())),
    )
    .await
}
