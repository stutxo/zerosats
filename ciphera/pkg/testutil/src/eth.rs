use std::{fs, path::PathBuf, process::Command, sync::Arc};

use once_cell::sync::Lazy;
use serde_json::json;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

fn find_eth() -> PathBuf {
    repo_root().join("ciphera/citrea")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn local_citrea_root() -> PathBuf {
    repo_root().join(".citrea").join(CITREA_VERSION)
}

fn citrea_path(env_name: &str, local_path: PathBuf, container_path: &str) -> PathBuf {
    std::env::var_os(env_name)
        .map(PathBuf::from)
        .or_else(|| local_path.exists().then_some(local_path))
        .unwrap_or_else(|| PathBuf::from(container_path))
}

const DEV_PRIVATE_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const CITREA_VERSION: &str = "v2.1.0";
const CITREA_DEVNET_PORT: u16 = 12345;

static CITREA_NODE_SEMAPHORE: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(1)));

/// Addresses deployed by the Hardhat deploy script
#[derive(Debug, Clone)]
pub struct DeployedAddresses {
    pub rollup_proxy: String,
    pub erc20: String,
}

#[derive(Debug)]
pub struct EthNode {
    process: Option<std::process::Child>,
    port: u16,
    options: EthNodeOptions,
    deployed: Option<DeployedAddresses>,
    _citrea_permit: Option<OwnedSemaphorePermit>,
}

impl Drop for EthNode {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Debug, Default)]
pub struct EthNodeOptions {
    pub use_noop_verifier: bool,
    pub use_deployer_as_pool_rollup: bool,
    pub validators: Option<Vec<String>>,
}

impl Default for EthNode {
    fn default() -> Self {
        Self::new(EthNodeOptions::default())
    }
}

impl EthNode {
    pub fn new(options: EthNodeOptions) -> Self {
        Self {
            process: None,
            port: CITREA_DEVNET_PORT,
            options,
            deployed: None,
            _citrea_permit: None,
        }
    }

    pub fn run(&mut self) {
        // This must be the actual Citrea dev bin instead of running it through yarn,
        // because we send a SIGKILL which yarn can't forward to the Citrea dev node.
        let local_citrea_root = local_citrea_root();
        let citrea_bin = citrea_path(
            "CIPHERA_TEST_CITREA_BIN",
            local_citrea_root.join("bin/citrea"),
            "/citrea",
        );
        let configs_root = citrea_path(
            "CIPHERA_TEST_CITREA_CONFIGS_ROOT",
            local_citrea_root.join("resources/configs"),
            "/configs",
        );
        let genesis_root = citrea_path(
            "CIPHERA_TEST_CITREA_GENESIS_ROOT",
            local_citrea_root.join("resources/genesis"),
            "/genesis",
        );
        let rollup_config_path = configs_root.join("mock/sequencer_rollup_config.toml");
        let sequencer_config_path = configs_root.join("mock/sequencer_config.toml");
        let genesis_path = genesis_root.join("mock");

        let mut command = Command::new(&citrea_bin);

        command.current_dir(find_eth());

        command.arg("--dev");
        command.arg("--da-layer").arg("mock");
        command.arg("--rollup-config-path").arg(rollup_config_path);
        command.arg("--sequencer").arg(sequencer_config_path);
        command.arg("--genesis-paths").arg(genesis_path);

        let should_log = std::env::var("LOG_CITREA_OUTPUT")
            .map(|v| v == "1")
            .unwrap_or(false);
        if !should_log {
            command
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
        }

        let process = command.spawn().unwrap_or_else(|err| {
            panic!(
                "Failed to start Citrea dev node: {err}. Tried binary: {}; configs root: {}; \
                 genesis root: {}. Set CIPHERA_TEST_CITREA_BIN, CIPHERA_TEST_CITREA_CONFIGS_ROOT, \
                 and CIPHERA_TEST_CITREA_GENESIS_ROOT, or run scripts/test.sh once to populate \
                 .citrea/{CITREA_VERSION}.",
                citrea_bin.display(),
                configs_root.display(),
                genesis_root.display()
            )
        });
        self.process = Some(process);
    }

    fn stop(&mut self) {
        if let Some(mut process) = self.process.take() {
            process.kill().expect("Failed to kill Citrea dev node");
            process
                .wait()
                .expect("Failed to wait for Citrea dev node to exit");
        }

        let resources_dir = find_eth().join("resources");
        if resources_dir.exists() {
            match fs::remove_dir_all(&resources_dir) {
                Ok(_) => println!("Successfully removed {}", resources_dir.display()),
                Err(e) => println!("Failed to remove {}: {e}", resources_dir.display()),
            }
        }
    }

    pub fn rpc_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Returns the deployed contract addresses (available after
    /// `run_and_deploy`)
    pub fn deployed(&self) -> &DeployedAddresses {
        self.deployed.as_ref().expect("Contracts not yet deployed")
    }

    async fn wait_for_healthy(&self) -> Result<u64, Box<dyn std::error::Error>> {
        let time_between_requests = std::time::Duration::from_millis(100);
        let max_retries = 1_000 / time_between_requests.as_millis() as usize;

        let client = reqwest::Client::new();

        for retry in 0..max_retries {
            let is_last_retry = retry == max_retries - 1;

            let req = client
                .post(self.rpc_url())
                .json(&json!({
                    "jsonrpc": "2.0",
                    "method": "eth_blockNumber",
                    "params": [],
                    "id": 1
                }))
                .build()?;

            match client.execute(req).await {
                Ok(res) if res.status().is_success() => {
                    if let Ok(body) = res.json::<serde_json::Value>().await {
                        if let Some(result) = body.get("result").and_then(|r| r.as_str()) {
                            let block_height =
                                u64::from_str_radix(result.trim_start_matches("0x"), 16)?;
                            return Ok(block_height);
                        }
                    }
                }
                Ok(res) if is_last_retry => {
                    return Err(format!("Failed to get block height: {}", res.status()).into());
                }
                Err(err) if is_last_retry => return Err(err.into()),
                _ => {}
            }

            tokio::time::sleep(time_between_requests).await;
        }

        Err("Max retries exceeded".into())
    }

    async fn wait_for_next_block(&self) -> Result<u64, Box<dyn std::error::Error>> {
        let current_block = self.wait_for_healthy().await?;
        loop {
            let block_height = self.wait_for_healthy().await?;
            if block_height > current_block {
                return Ok(block_height);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    fn deploy(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let verifier = self.deploy_verifier()?;
        let mut command = Command::new("node_modules/.bin/hardhat");

        command.current_dir(find_eth());

        self.apply_devnet_env(&mut command);
        command.env("SECRET_KEY", DEV_PRIVATE_KEY);
        command.env("TESTING_URL", self.rpc_url());
        command.env("VERIFIER", verifier);

        if self.options.use_noop_verifier {
            command.env("DEV_USE_NOOP_VERIFIER", "1");
        }

        if self.options.use_deployer_as_pool_rollup {
            command.env("DEV_USE_DEPLOYER_AS_POOL_ROLLUP", "1");
        }

        if let Some(validators) = &self.options.validators {
            command.env("VALIDATORS", validators.join(","));
        }

        command.arg("run");
        command.arg("scripts/deploy.ts");

        let should_log = std::env::var("LOG_HARDHAT_DEPLOY_OUTPUT")
            .map(|v| v == "1")
            .unwrap_or(false);

        // Always capture stdout (for DEPLOY_OUTPUT parsing) and stderr (for failure
        // diagnostics)
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        let process = command.spawn().expect("Failed to start Citrea deploy");
        let output = process.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Always replay on failure so deploy errors are visible;
        // on success only replay when logging is requested.
        if !output.status.success() || should_log {
            eprint!("{stdout}");
            eprint!("{stderr}");
        }

        if !output.status.success() {
            return Err("Citrea deploy returned a non-zero exit code".into());
        }

        // Parse DEPLOY_OUTPUT={"rollupProxy":"0x...","erc20":"0x...",...}
        for line in stdout.lines() {
            if let Some(json_str) = line.strip_prefix("DEPLOY_OUTPUT=") {
                let v: serde_json::Value =
                    serde_json::from_str(json_str).expect("Failed to parse DEPLOY_OUTPUT JSON");
                self.deployed = Some(DeployedAddresses {
                    rollup_proxy: v["rollupProxy"]
                        .as_str()
                        .expect("missing rollupProxy")
                        .to_string(),
                    erc20: v["erc20"].as_str().expect("missing erc20").to_string(),
                });
                return Ok(());
            }
        }

        Err("Deploy script did not output DEPLOY_OUTPUT line".into())
    }

    fn apply_devnet_env(&self, command: &mut Command) {
        let rpc_url = self.rpc_url();
        command.env("NETWORK", "dev");
        command.env("RPC_URL", &rpc_url);
        command.env("TESTING_URL", rpc_url);
        command.env("PRIVATE_KEY", DEV_PRIVATE_KEY);
    }

    fn deploy_verifier(&self) -> Result<String, Box<dyn std::error::Error>> {
        let mut command = Command::new("node_modules/.bin/hardhat");
        command.current_dir(find_eth());
        self.apply_devnet_env(&mut command);

        command.arg("run");
        if self.options.use_noop_verifier {
            command.arg("scripts/devnet/deploy-verifiers-devnet.ts");
        } else {
            command.arg("scripts/deploy-verifier.ts");
        }

        let should_log = std::env::var("LOG_HARDHAT_DEPLOY_OUTPUT")
            .map(|v| v == "1")
            .unwrap_or(false);

        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        let process = command.spawn().expect("Failed to start verifier deploy");
        let output = process.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() || should_log {
            eprint!("{stdout}");
            eprint!("{stderr}");
        }

        if !output.status.success() {
            return Err("Verifier deploy returned a non-zero exit code".into());
        }

        for line in stdout.lines() {
            if let Some(address) = line.strip_prefix("VERIFIER=") {
                return Ok(address.to_string());
            }
        }

        Err("Verifier deploy did not output VERIFIER line".into())
    }

    pub async fn run_and_deploy(mut self) -> Arc<Self> {
        self._citrea_permit = Some(
            Arc::clone(&CITREA_NODE_SEMAPHORE)
                .acquire_owned()
                .await
                .expect("Citrea node semaphore closed"),
        );

        let eth_node = tokio::task::spawn_blocking(move || {
            self.run();
            self
        })
        .await
        .unwrap();

        eth_node
            .wait_for_next_block()
            .await
            .expect("Failed to wait for Citrea node");

        let mut eth_node = eth_node;
        let eth_node = tokio::task::spawn_blocking(move || {
            // Deploy is flaky
            for i in 0..3 {
                match eth_node.deploy() {
                    Ok(_) => break,
                    Err(err) => {
                        if i == 2 {
                            panic!(
                                "Failed to deploy contracts: {err:?}; Run with \
                                 LOG_HARDHAT_DEPLOY_OUTPUT=1 to see the output"
                            );
                        } else {
                            std::thread::sleep(std::time::Duration::from_secs(5));
                        }
                    }
                }
            }

            eth_node
        })
        .await
        .unwrap();

        Arc::new(eth_node)
    }
}
