// rcli csv -i input.csv -o output.json --header -d ','
//
// rcli genpass -l 20
//
// rcli base64 encode -i Cargo.toml
// rcli base64 decode -i fixtures/b64.txt --format urlsafe
//
// rcli http serve -d ./fixtures -p 8080               # serve fixtures directory over HTTP
// curl -sS http://127.0.0.1:8080/juventus.csv | head  # return 200 and first 10 lines of juventus.csv
// curl -i http://127.0.0.1:8080/no-such-file.txt      # return 404
//
// rcli text generate --format blake3 -o ./fixtures       # generate blake3 key
// rcli text sign -i Cargo.toml -k fixtures/blake3.txt    # using blake3 key to sign Cargo.toml
// rcli text verify -i Cargo.toml -k fixtures/blake3.txt --sig <output from sign>

use clap::Parser;
use rcli::{CmdExector, Opts};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let opts = Opts::parse();
    opts.cmd.execute().await?;

    Ok(())
}
