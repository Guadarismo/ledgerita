use ledgerita_core::network::run_p2p_node;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("💎Iniciando Ledgerita Protocol Core...");
    run_p2p_node().await?;
    Ok(())
}
