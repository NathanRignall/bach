//! `cargo run --example replay -- <claude|codex|opencode> <file.jsonl>`
//! Prints the normalized AgentEvents for a recorded CLI stream, one JSON object per line.
use bach_lib::adapters::AgentKind;

fn main() {
    let mut args = std::env::args().skip(1);
    let kind: AgentKind = serde_json::from_value(args.next().expect("agent").into()).expect("claude|codex|opencode");
    let text = std::fs::read_to_string(args.next().expect("file")).expect("read");
    for ev in text.lines().flat_map(|l| kind.parse_line(l)) {
        println!("{}", serde_json::to_string(&ev).unwrap());
    }
}
