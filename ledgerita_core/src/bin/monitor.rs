use ledgerita_core::state::State;
use ed25519_dalek::SigningKey;
use ledgerita_core::crypto::Address;

fn main() {
    println!("===========================================");
    println!(" 🔍 [Ledgerita Explorer] Supply Monitor");
    println!("===========================================\n");
    
    // 1. Simular la conexión y descarga del estado global de la red
    println!("[*] Conectando con el Core y analizando el BTreeMap...");
    let mut state = State::new();
    
    // Simular que la red ha liberado fondos durante varias épocas
    let key = SigningKey::from_bytes(&[1u8; 32]);
    let addr = Address::new(key.verifying_key());
    state.mint(addr, 15_500_000); // 15.5 Millones emitidos al nodo validador A
    
    let key2 = SigningKey::from_bytes(&[2u8; 32]);
    let addr2 = Address::new(key2.verifying_key());
    state.mint(addr2, 2_000_000); // 2 Millones emitidos al nodo validador B

    // 2. El script de exploración llama la función determinista del Core
    let circulating_supply = state.total_supply();
    let hard_cap = 21_000_000;
    
    println!("📊 Suministro Circulante Actual : {} LIT", circulating_supply);
    println!("🔒 Límite Matemático (Hard Cap) : {} LIT", hard_cap);
    
    // 3. Lógica de Alertas Visuales (Totalmente externa, el Core no se entera)
    if circulating_supply > hard_cap {
        println!("\n🚨 ALERTA ROJA: INFLACIÓN DETECTADA. REGLA DE 21M ROTA.");
    } else {
        println!("\n✅ Estado de la Red: Saludable. Reglas económicas intactas.");
        let remaining = hard_cap - circulating_supply;
        println!("⏳ LIT restantes en el Tesoro por minar: {}", remaining);
    }
}
