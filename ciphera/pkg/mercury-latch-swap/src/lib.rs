use clap::Parser;
use color_eyre::Result;
use serde::Serialize;

mod cli;
mod commands;
mod latch;
mod mercury;
mod state;

use cli::Cli;

pub async fn run_from_args() -> Result<()> {
    let cli = Cli::parse();
    let output = commands::execute(cli.command).await?;
    print_json(&output)
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
