use std::collections::HashMap;
use std::fs;
use std::io::{stdout, Write};
use std::time::Instant;

use crossterm::cursor::MoveTo;
use crossterm::event::{read, Event, KeyCode, KeyEvent};
use crossterm::queue;
use crossterm::style::{Print, SetForegroundColor, ResetColor, Color};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};

struct HighlightRange {
    row: u16,
    left: u16,
    right: u16,
}

struct Stats {
    total_chars: usize,
    correct_chars: usize,
    start: Instant,
    sentences_done: usize,
}

fn load_lines(path: &str) -> Vec<String> {
    let content = fs::read_to_string(path).unwrap_or_else(|_| {
        eprintln!("Error: cannot read file '{}'", path);
        std::process::exit(1);
    });
    content.lines().filter(|l| !l.trim().is_empty()).map(|l| l.to_string()).collect()
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
                let label_start = i;
                while i < bytes.len() && bytes[i] != b'|' {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'|' {
                    let pipe_right = i as u16;
                    let label_str = String::from_utf8_lossy(&bytes[label_start..i]);
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

fn main() -> crossterm::Result<()> {
    let quotes = load_lines("quotes.txt");
    if quotes.is_empty() {
        eprintln!("Error: quotes.txt is empty");
        std::process::exit(1);
    }

    let keyboard_lines = load_lines("keyboard.txt");
    if keyboard_lines.is_empty() {
        eprintln!("Error: keyboard.txt is empty");
        std::process::exit(1);
    }

    let highlight_map = build_highlight_map(&keyboard_lines);

    enable_raw_mode()?;
    let mut stdout = stdout();

    let mut rng = fastrand::Rng::new();
    let mut stats = Stats {
        total_chars: 0,
        correct_chars: 0,
        start: Instant::now(),
        sentences_done: 0,
    };

    loop {
        let text = &quotes[rng.usize(0..quotes.len())];
        let target_chars: Vec<char> = text.chars().collect();
        let mut typed: Vec<char> = Vec::new();

        loop {
            render_ui(&mut stdout, &target_chars, &typed, &stats, &keyboard_lines, &highlight_map)?;
            stdout.flush()?;

            if let Event::Key(KeyEvent { code, .. }) = read()? {
                match code {
                    KeyCode::Char(c) => {
                        if typed.len() < target_chars.len() {
                            typed.push(c);
                        }
                    }
                    KeyCode::Backspace => {
                        if !typed.is_empty() {
                            typed.pop();
                        }
                    }
                    KeyCode::Esc => {
                        disable_raw_mode()?;
                        print_stats(&stats);
                        return Ok(());
                    }
                    _ => {}
                }
            }

            if typed.len() >= target_chars.len() {
                stats.total_chars += target_chars.len();
                stats.correct_chars += typed.iter().zip(target_chars.iter()).filter(|(a, b)| **a == **b).count();
                stats.sentences_done += 1;
                break;
            }
        }
    }
}

fn print_stats(stats: &Stats) {
    let elapsed = stats.start.elapsed().as_secs_f64();
    let accuracy = if stats.total_chars > 0 {
        stats.correct_chars as f64 / stats.total_chars as f64 * 100.0
    } else {
        0.0
    };
    let cpm = if elapsed > 0.0 {
        stats.total_chars as f64 / elapsed * 60.0
    } else {
        0.0
    };

    println!("\n  Done! Final Statistics:");
    println!("  Sentences completed: {}", stats.sentences_done);
    println!("  Total characters typed: {}", stats.total_chars);
    println!("  Correct: {}", stats.correct_chars);
    println!("  Accuracy: {:.1}%", accuracy);
    println!("  Speed: {:.0} CPM", cpm);
}

fn render_ui<W: Write>(
    out: &mut W,
    target_chars: &[char],
    typed: &[char],
    stats: &Stats,
    keyboard_lines: &[String],
    highlight_map: &HashMap<char, Vec<HighlightRange>>,
) -> crossterm::Result<()> {
    queue!(out, Clear(ClearType::All))?;

    let mut y: u16 = 1;

    let accuracy = if stats.total_chars > 0 {
        stats.correct_chars as f64 / stats.total_chars as f64 * 100.0
    } else {
        100.0
    };
    let header = format!(
        "  Sentences: {} | Accuracy: {:.1}% | Characters: {}",
        stats.sentences_done, accuracy, stats.total_chars
    );
    queue!(out, MoveTo(2, y), Print(&header))?;
    y += 2;

    let text: String = target_chars.iter().collect();
    let display = if text.len() > 48 {
        format!("{}…", &text[..48])
    } else {
        text.clone()
    };

    let book_lines: Vec<String> = vec![
        "   ___________________________________________________".into(),
        "  /                                                   \\".into(),
        format!(" |  {:<49}|", display),
        "  \\___________________________________________________/".into(),
    ];
    for line in &book_lines {
        queue!(out, MoveTo(2, y), Print(line))?;
        y += 1;
    }
    y += 1;

    for (i, &tc) in target_chars.iter().enumerate() {
        let x = 3 + i as u16;
        if i < typed.len() {
            let t = typed[i];
            let color = if t == tc { Color::Green } else { Color::Red };
            queue!(out, MoveTo(x, y), SetForegroundColor(color), Print(t))?;
        } else {
            queue!(out, MoveTo(x, y), Print(tc))?;
        }
    }
    queue!(out, ResetColor)?;
    y += 2;

    for (ri, line) in keyboard_lines.iter().enumerate() {
        queue!(out, MoveTo(2, y + ri as u16), Print(line))?;
    }

    if let Some(nk) = target_chars.get(typed.len()) {
        let nk_lower = nk.to_ascii_lowercase();
        if let Some(ranges) = highlight_map.get(&nk_lower) {
            for r in ranges {
                let line = &keyboard_lines[r.row as usize];
                let bytes = line.as_bytes();
                queue!(out, SetForegroundColor(Color::Yellow))?;
                for cx in r.left..=r.right {
                    queue!(out, MoveTo(cx + 2, y + r.row))?;
                    queue!(out, Print(bytes[cx as usize] as char))?;
                }
                queue!(out, ResetColor)?;
            }
        }
    }

    let footer_y = y + keyboard_lines.len() as u16 + 1;
    queue!(out, MoveTo(2, footer_y), SetForegroundColor(Color::DarkGrey), Print("[Esc] to quit | [Backspace] to correct"), ResetColor)?;

    Ok(())
}
