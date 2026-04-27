use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use fastrand::Rng;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Clone, Serialize, Deserialize)]
struct HighlightRange {
    row: u16,
    left: u16,
    right: u16,
}

struct GameState {
    words: Vec<String>,
    dialogues: Vec<String>,
    keyboard_lines: Vec<String>,
    highlight_map: HashMap<char, Vec<HighlightRange>>,
}

#[derive(Serialize)]
struct GameData {
    target_text: String,
    typed: Vec<char>,
    total_chars: usize,
    correct_chars: usize,
    sentences_done: usize,
    backspace_count: usize,
    elapsed_secs: f64,
    highlight_ranges: Vec<HighlightRange>,
    keyboard_lines: Vec<String>,
    mode: String,
    done: bool,
}

#[derive(Deserialize)]
struct ClientInput {
    key: String,
    mode: Option<String>,
}

fn load_lines(path: &str) -> Vec<String> {
    let content = fs::read_to_string(path).unwrap_or_else(|_| {
        eprintln!("Error: cannot read file '{}'", path);
        std::process::exit(1);
    });
    content.lines().filter(|l| !l.trim().is_empty()).map(|l| l.to_string()).collect()
}

fn pick_dialogue(dialogues: &[String], rng: &mut Rng) -> String {
    let total = dialogues.len();
    if total == 0 {
        return String::new();
    }
    let odd_count = (total + 1) / 2;
    let idx = rng.usize(0..odd_count) * 2;
    let line1 = &dialogues[idx];
    let line2 = dialogues.get(idx + 1).map(|s| s.as_str()).unwrap_or("");
    if line2.is_empty() {
        line1.clone()
    } else {
        format!("{}\n{}", line1, line2)
    }
}

fn pick_words(words: &[String], rng: &mut Rng) -> String {
    let total = words.len();
    if total == 0 {
        return String::new();
    }
    let count = 10.min(total);
    let start = rng.usize(0..total - count + 1);
    words[start..start + count].join(" ")
}

fn build_highlight_map(keyboard_lines: &[String]) -> HashMap<char, Vec<HighlightRange>> {
    let mut map: HashMap<char, Vec<HighlightRange>> = HashMap::new();
    for (ri, line) in keyboard_lines.iter().enumerate() {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'|' {
                let pipe_left = i as u16;
                i += 1;
                while i < bytes.len() && bytes[i] != b'|' {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'|' {
                    let pipe_right = i as u16;
                    let label_str = String::from_utf8_lossy(&bytes[pipe_left as usize + 1..i]);
                    let trimmed = label_str.trim();
                    if trimmed.len() == 1 {
                        let ch = trimmed.chars().next().unwrap().to_ascii_lowercase();
                        map.entry(ch).or_default().push(HighlightRange {
                            row: ri as u16,
                            left: pipe_left,
                            right: pipe_right,
                        });
                    } else if trimmed == "Space" {
                        map.entry(' ').or_default().push(HighlightRange {
                            row: ri as u16,
                            left: pipe_left,
                            right: pipe_right,
                        });
                    }
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
    }
    map
}

#[tokio::main]
async fn main() {
    let words = load_lines("quotes.txt");
    let dialogues = load_lines("dialogue.txt");
    let keyboard_lines = load_lines("keyboard.txt");
    let highlight_map = build_highlight_map(&keyboard_lines);

    let state = Arc::new(GameState {
        words,
        dialogues,
        keyboard_lines,
        highlight_map,
    });

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/keyboard.txt", get(keyboard_file_handler))
        .route("/quotes.txt", get(quotes_file_handler))
        .route("/dialogue.txt", get(dialogue_file_handler))
        .route("/typing.ico", get(typing_ico_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:30001").await.unwrap();
    println!("Server running at http://0.0.0.0:30001");
    axum::serve(listener, app).await.unwrap();
}

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn typing_ico_handler() -> impl axum::response::IntoResponse {
    (
        [("Content-Type", "image/x-icon")],
        include_bytes!("../static/typing.ico"),
    )
}

async fn keyboard_file_handler(State(state): State<Arc<GameState>>) -> String {
    state.keyboard_lines.join("\n")
}

async fn quotes_file_handler(State(state): State<Arc<GameState>>) -> String {
    state.words.join("\n")
}

async fn dialogue_file_handler(State(state): State<Arc<GameState>>) -> String {
    state.dialogues.join("\n")
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<GameState>>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<GameState>) {
    let (sender, mut receiver) = socket.split();
    let sender = Arc::new(Mutex::new(sender));

    let mut typed: Vec<char> = Vec::new();
    let mut target_text: String;
    let mut target_chars: Vec<char>;
    let mut total_chars: usize = 0;
    let mut correct_chars: usize = 0;
    let mut sentences_done: usize = 0;
    let mut backspace_count: usize = 0;
    let start = Instant::now();
    let mut rng = Rng::new();
    let mut mode = String::from("words");

    fn next_target(mode: &str, words: &[String], dialogues: &[String], rng: &mut Rng) -> String {
        match mode {
            "dialogue" => pick_dialogue(dialogues, rng),
            _ => pick_words(words, rng),
        }
    }

    target_text = next_target(&mode, &state.words, &state.dialogues, &mut rng);
    target_chars = target_text.chars().collect();

    send_game_data(&sender, &state, &target_text, &target_chars, &typed, total_chars, correct_chars, sentences_done, backspace_count, &start, &mode, false).await;

    while let Some(msg) = receiver.next().await {
        if let Ok(Message::Text(text)) = msg {
            if let Ok(input) = serde_json::from_str::<ClientInput>(&text) {
                // Mode switch
                if let Some(new_mode) = &input.mode {
                    if new_mode != &mode {
                        mode = new_mode.clone();
                        typed.clear();
                        target_text = next_target(&mode, &state.words, &state.dialogues, &mut rng);
                        target_chars = target_text.chars().collect();
                        total_chars = 0;
                        correct_chars = 0;
                        sentences_done = 0;
                        backspace_count = 0;
                        send_game_data(&sender, &state, &target_text, &target_chars, &typed, total_chars, correct_chars, sentences_done, backspace_count, &start, &mode, false).await;
                        continue;
                    }
                }
                match input.key.as_str() {
                    "Esc" => {
                        send_game_data(&sender, &state, &target_text, &target_chars, &typed, total_chars, correct_chars, sentences_done, backspace_count, &start, &mode, true).await;
                        let _ = sender.lock().await.close().await;
                        return;
                    }
                    "Backspace" => {
                        if !typed.is_empty() {
                            typed.pop();
                            backspace_count += 1;
                        }
                    }
                    c if c.len() == 1 => {
                        let ch = c.chars().next().unwrap();
                        if typed.len() < target_chars.len() {
                            typed.push(ch);
                        }
                        if typed.len() >= target_chars.len() {
                            total_chars += target_chars.len();
                            correct_chars += typed.iter().zip(target_chars.iter()).filter(|(a, b)| **a == **b).count();
                            sentences_done += 1;
                            typed.clear();
                            target_text = next_target(&mode, &state.words, &state.dialogues, &mut rng);
                            target_chars = target_text.chars().collect();
                        }
                    }
                    _ => {}
                }
                send_game_data(&sender, &state, &target_text, &target_chars, &typed, total_chars, correct_chars, sentences_done, backspace_count, &start, &mode, false).await;
            }
        } else if let Ok(Message::Close(_)) = msg {
            break;
        }
    }
}

async fn send_game_data(
    sender: &Arc<Mutex<SplitSink<WebSocket, Message>>>,
    state: &GameState,
    target_text: &str,
    target_chars: &[char],
    typed: &[char],
    total_chars: usize,
    correct_chars: usize,
    sentences_done: usize,
    backspace_count: usize,
    start: &Instant,
    mode: &str,
    done: bool,
) {
    let nk = if !done && typed.len() < target_chars.len() {
        Some(&target_chars[typed.len()])
    } else {
        None
    };

    let highlight_ranges: Vec<HighlightRange> = nk
        .and_then(|c| state.highlight_map.get(&c.to_ascii_lowercase()))
        .cloned()
        .unwrap_or_default();

    let data = GameData {
        target_text: target_text.to_string(),
        typed: typed.to_vec(),
        total_chars,
        correct_chars,
        sentences_done,
        backspace_count,
        elapsed_secs: start.elapsed().as_secs_f64(),
        highlight_ranges,
        keyboard_lines: state.keyboard_lines.clone(),
        mode: mode.to_string(),
        done,
    };

    if let Ok(json) = serde_json::to_string(&data) {
        let _ = sender.lock().await.send(Message::Text(json.into())).await;
    }
}
