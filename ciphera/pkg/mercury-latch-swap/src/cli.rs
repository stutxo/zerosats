use std::path::PathBuf;

use clap::{Args as ClapArgs, Parser, Subcommand};

pub(crate) const DEFAULT_BTC_EXPLORER: &str = "https://mempool.space";
pub(crate) const DEFAULT_CITREA_CHAIN: u64 = 5115;
pub(crate) const DEFAULT_REFUND_BLOCKS: u64 = 2;
pub(crate) const DEFAULT_TICKER: &str = "WCBTC";
pub(crate) const DEFAULT_ZEROSATS_HOST: &str = "https://ciphera.satsbridge.com";

#[derive(Parser, Debug, Clone)]
#[command(name = "mercury-latch-swap")]
#[command(about = "Two-party Mercury/Zerosats latch swap protocol")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum Command {
    /// Show safe local progress and the next protocol action.
    Status(StateCommandArgs),
    /// Inspect the public swap transcript before taking the next action.
    InspectSwap(SwapCommandArgs),
    /// Seller-facing phase commands for an operator run.
    #[command(subcommand)]
    Seller(SellerCommand),
    /// Buyer-facing phase commands for an operator run.
    #[command(subcommand)]
    Buyer(BuyerCommand),
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum SellerCommand {
    /// Create local seller state.
    Init(SellerInitArgs),
    /// Create the Mercury offer and update swap.json.
    Offer(SwapStateCommandArgs),
    /// Import buyer addresses, create the latch template, and update swap.json.
    Template(SwapStateCommandArgs),
    /// Import buyer funding, verify it, transfer/unlock Mercury, and update
    /// swap.json.
    Release(SellerReleaseArgs),
    /// Import buyer receive confirmation, retrieve the preimage, and claim the
    /// Zerosats latch.
    Claim(SwapStateCommandArgs),
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum BuyerCommand {
    /// Import the seller offer, create buyer addresses, and update swap.json.
    Accept(BuyerAcceptArgs),
    /// Import seller template, fund the latch, and update swap.json.
    Fund(SwapStateCommandArgs),
    /// Import seller unlock, receive Mercury, and update swap.json.
    Receive(SwapStateCommandArgs),
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct StateCommandArgs {
    #[arg(long, env = "SWAP_STATE")]
    pub(crate) state: PathBuf,
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct SwapCommandArgs {
    #[arg(long, env = "SWAP_JSON")]
    pub(crate) swap: PathBuf,
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct SwapStateCommandArgs {
    #[arg(long, env = "SWAP_STATE")]
    pub(crate) state: PathBuf,

    #[command(flatten)]
    pub(crate) transcript: SwapCommandArgs,
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct SellerReleaseArgs {
    #[command(flatten)]
    pub(crate) swap: SwapStateCommandArgs,

    /// Acknowledge this phase releases and unlocks the Mercury transfer.
    #[arg(long = "release-mercury")]
    pub(crate) release_mercury: bool,
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct BuyerAcceptArgs {
    #[arg(long, env = "SWAP_STATE")]
    pub(crate) state: PathBuf,

    #[command(flatten)]
    pub(crate) transcript: SwapCommandArgs,

    #[command(flatten)]
    pub(crate) local: LocalWalletArgs,
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct ZerosatsHostArgs {
    #[arg(long, env = "CIPHERA_HOST", default_value = DEFAULT_ZEROSATS_HOST)]
    pub(crate) zerosats_host: String,
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct LatchSettingsArgs {
    #[arg(long, env = "CITREA_CHAIN", default_value_t = DEFAULT_CITREA_CHAIN)]
    pub(crate) citrea_chain: u64,

    #[arg(long, env = "REFUND_BLOCKS", default_value_t = DEFAULT_REFUND_BLOCKS)]
    pub(crate) refund_blocks: u64,

    #[arg(long, env = "CIPHERA_TICKER", default_value = DEFAULT_TICKER)]
    pub(crate) ticker: String,

    #[arg(long, env = "BTC_EXPLORER", default_value = DEFAULT_BTC_EXPLORER)]
    pub(crate) btc_explorer: String,
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct LocalWalletArgs {
    #[arg(long, env = "MERCURY_SETTINGS_FILE")]
    pub(crate) mercury_settings_file: PathBuf,

    #[arg(long, env = "MERCURY_WALLET")]
    pub(crate) mercury_wallet: String,

    #[arg(long, env = "ZEROSATS_WALLET")]
    pub(crate) zerosats_wallet: String,

    #[arg(long, env = "ZEROSATS_WALLET_DIR")]
    pub(crate) zerosats_wallet_dir: Option<PathBuf>,

    #[arg(long, env = "SWAP_OUTPUT_PREFIX")]
    pub(crate) output_prefix: PathBuf,
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct SellerInitArgs {
    #[arg(long, env = "SWAP_STATE")]
    pub(crate) state: PathBuf,

    #[arg(long, env = "STATECHAIN_ID")]
    pub(crate) statechain_id: String,

    #[arg(long, env = "AMOUNT_SAT")]
    pub(crate) amount_sat: u64,

    #[arg(long, env = "MERCURY_AMOUNT_SAT")]
    pub(crate) mercury_amount_sat: u64,

    #[command(flatten)]
    pub(crate) local: LocalWalletArgs,

    #[command(flatten)]
    pub(crate) node: ZerosatsHostArgs,

    #[command(flatten)]
    pub(crate) latch: LatchSettingsArgs,
}
