use core::{panic, time};
use std::{sync::Arc, time::Duration};

use post_service::operator::{self, ServiceState};
use tokio::{net::TcpListener, time::sleep};

use post::{
    initialize::{CpuInitializer, Initialize},
    pow::randomx::RandomXFlag,
};
use post_service::client::spacemesh_v1::{service_response, GenProofStatus};

#[allow(dead_code)]
mod server;
use server::TestServer;

#[tokio::test]
async fn test_gen_proof_in_progress() {
    // Initialize some data
    let datadir = tempfile::tempdir().unwrap();

    let cfg = post::config::ProofConfig {
        k1: 8,
        k2: 12,
        pow_difficulty: [0xFF; 32],
    };
    let init_cfg = post::config::InitConfig {
        min_num_units: 1,
        max_num_units: 100,
        labels_per_unit: 2560,
        scrypt: post::config::ScryptParams::new(2, 1, 1),
    };

    CpuInitializer::new(init_cfg.scrypt)
        .initialize(
            datadir.path(),
            &[0xBE; 32],
            &[0xCE; 32],
            init_cfg.labels_per_unit,
            4,
            256,
            None,
        )
        .unwrap();

    let pow_flags = RandomXFlag::get_recommended_flags();

    let service = Arc::new(
        post_service::service::PostService::new(
            datadir.into_path(),
            cfg,
            init_cfg,
            16,
            post::config::Cores::Any(1),
            pow_flags,
        )
        .unwrap(),
    );

    let mut test_server = TestServer::new().await;
    let client = test_server.create_client(service.clone(), None);
    tokio::spawn(client.run(None, time::Duration::from_secs(1)));

    // Create operator server and client
    let listener = TcpListener::bind("localhost:0").await.unwrap();
    let operator_addr = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(operator::run(listener, service));

    let status_url = format!("{operator_addr}/status");
    let resp = reqwest::get(&status_url).await.unwrap();
    let status = resp.json().await.unwrap();
    // It starts in idle state
    assert!(matches!(status, ServiceState::Idle));

    // It transforms to Proving when a proof generation starts
    let connected = test_server.connected.recv().await.unwrap();

    loop {
        let response = TestServer::generate_proof(&connected, vec![0xCA; 32]).await;
        let status_resp = reqwest::get(&status_url).await.unwrap();
        let status = status_resp.json().await.unwrap();

        if let Some(service_response::Kind::GenProof(resp)) = response.kind {
            match resp.status() {
                GenProofStatus::Ok => {
                    if resp.proof.is_some() {
                        assert!(matches!(status, ServiceState::Idle));
                        break;
                    }
                    assert!(matches!(
                        status,
                        ServiceState::Proving { .. } | ServiceState::DoneProving
                    ));
                }
                _ => {
                    panic!("got error response");
                }
            }
        } else {
            panic!("got wrong response kind");
        }
        sleep(Duration::from_millis(10)).await;
    }
}
