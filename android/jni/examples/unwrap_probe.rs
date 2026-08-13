//! Diagnostic: decrypt a gift-wrap addressed to a machine whose secret we hold
//! (on-metal debugging — the wrap JSON on stdin, the machine sk hex as argv).
//!   cargo run --example unwrap_probe --features mock -- <machine_sk_hex> < wrap.json
use charter_transport::nip59;

fn main() {
    let sk_hex = std::env::args().nth(1).expect("machine sk hex");
    let mut sk = [0u8; 32];
    for (i, b) in sk.iter_mut().enumerate() {
        *b = u8::from_str_radix(&sk_hex[i * 2..i * 2 + 2], 16).expect("hex");
    }
    let mut json = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut json).expect("stdin");
    for line in json.lines().filter(|l| !l.trim().is_empty()) {
        let wrap: charter_primitives::NostrEvent = serde_json::from_str(line).expect("wrap json");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        println!(
            "wrap id={} created_at={}",
            wrap.id.to_hex(),
            wrap.created_at
        );
        match nip59::unwrap_with_author(&wrap, &sk, now, nip59::MAX_JITTER_SECS) {
            Ok((rumor, author)) => {
                println!("  seal author (sender) = {}", author.to_hex());
                println!("  inner kind = {}", rumor.kind);
                println!("  inner created_at = {}", rumor.created_at);
                println!("  inner content = {}", rumor.content);
            }
            Err(e) => println!("  unwrap FAILED: {e:?}"),
        }
    }
}
