mod artifacts;
mod cancel;
mod catalog;
mod hashing;
mod jobs;
mod provenance;
mod service;
mod state;
mod upstream;
mod verify;

use apkw_proto::apkw::v1::toolchain_service_server::ToolchainServiceServer;
use apkw_util::serve_grpc_with_telemetry;
use service::Svc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    apkw_util::promote_legacy_env();
    serve_grpc_with_telemetry(
        "apkw-toolchain",
        env!("CARGO_PKG_VERSION"),
        "toolchain",
        "APKW_TOOLCHAIN_ADDR",
        apkw_util::DEFAULT_TOOLCHAIN_ADDR,
        |server| server.add_service(ToolchainServiceServer::new(Svc::default())),
    )
    .await
}
