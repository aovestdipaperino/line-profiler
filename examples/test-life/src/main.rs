use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{Write, stdout};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static START_TIME_MS: AtomicU64 = AtomicU64::new(0);

const WIDTH: usize = 20;
const HEIGHT: usize = 20;

struct Grid {
    cells: [[bool; WIDTH]; HEIGHT],
    previous: [[bool; WIDTH]; HEIGHT],
}

impl Grid {
    fn new_random() -> Self {
        let mut cells = [[false; WIDTH]; HEIGHT];
        let mut seed: u64 = 0xDEADBEEF;

        for row in 0..HEIGHT {
            for col in 0..WIDTH {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                cells[row][col] = (seed >> 63) == 1;
            }
        }

        Grid {
            cells,
            previous: [[false; WIDTH]; HEIGHT],
        }
    }

    fn count_neighbors(&self, row: usize, col: usize) -> u8 {
        let mut count = 0u8;

        for dr in [-1i32, 0, 1] {
            for dc in [-1i32, 0, 1] {
                if dr == 0 && dc == 0 {
                    continue;
                }

                let nr = (row as i32 + dr).rem_euclid(HEIGHT as i32) as usize;
                let nc = (col as i32 + dc).rem_euclid(WIDTH as i32) as usize;

                if self.cells[nr][nc] {
                    count += 1;
                }
            }
        }

        count
    }

    fn is_newly_spawned(&self, row: usize, col: usize) -> bool {
        self.cells[row][col] && !self.previous[row][col]
    }

    fn step(&mut self) {
        self.previous = self.cells;
        let mut next = [[false; WIDTH]; HEIGHT];

        for row in 0..HEIGHT {
            for col in 0..WIDTH {
                let neighbors = self.count_neighbors(row, col);
                let alive = self.cells[row][col];

                next[row][col] = match (alive, neighbors) {
                    (true, 2) | (true, 3) => true,
                    (false, 3) => true,
                    _ => false,
                };
            }
        }

        self.cells = next;
    }

    fn display(&self, generation: u64) {
        let mut out = stdout();

        execute!(out, MoveTo(0, 0)).unwrap();

        let header = format!(
            "Game of Life - Generation {} (Ctrl+C to exit)\n\n",
            generation
        );
        execute!(out, Print(&header)).unwrap();

        let mut line = String::with_capacity(WIDTH + 3);

        line.push('+');
        for _ in 0..WIDTH {
            line.push('-');
        }
        line.push_str("+\n");
        execute!(out, Print(&line)).unwrap();

        for row in 0..HEIGHT {
            execute!(out, Print("|")).unwrap();
            for col in 0..WIDTH {
                if self.cells[row][col] {
                    if self.is_newly_spawned(row, col) {
                        execute!(
                            out,
                            SetForegroundColor(Color::Green),
                            Print("#"),
                            ResetColor
                        )
                        .unwrap();
                    } else {
                        execute!(out, Print("#")).unwrap();
                    }
                } else {
                    execute!(out, Print(" ")).unwrap();
                }
            }
            execute!(out, Print("|\n")).unwrap();
        }

        line.clear();
        line.push('+');
        for _ in 0..WIDTH {
            line.push('-');
        }
        line.push_str("+\n");
        execute!(out, Print(&line)).unwrap();

        let alive_count = self.count_alive();
        let footer = format!("\nAlive cells: {}   ", alive_count);
        execute!(out, Print(&footer)).unwrap();

        out.flush().unwrap();
    }

    fn count_alive(&self) -> usize {
        let mut count = 0;
        for row in 0..HEIGHT {
            for col in 0..WIDTH {
                if self.cells[row][col] {
                    count += 1;
                }
            }
        }
        count
    }
}

fn main() {
    let mut out = stdout();

    execute!(out, EnterAlternateScreen, Hide, Clear(ClearType::All)).unwrap();

    let start_time = Instant::now();
    START_TIME_MS.store(0, Ordering::SeqCst);

    ctrlc::set_handler(move || {
        let elapsed_ms = START_TIME_MS.load(Ordering::SeqCst);
        let elapsed_secs = elapsed_ms / 1000;
        let hours = elapsed_secs / 3600;
        let minutes = (elapsed_secs % 3600) / 60;
        let seconds = elapsed_secs % 60;

        let mut out = stdout();
        execute!(out, Show, LeaveAlternateScreen).unwrap();
        println!("\nElapsed time: {:02}:{:02}:{:02}", hours, minutes, seconds);
        std::process::exit(0);
    })
    .ok();

    let mut grid = Grid::new_random();
    let mut generation: u64 = 0;

    loop {
        START_TIME_MS.store(start_time.elapsed().as_millis() as u64, Ordering::SeqCst);
        grid.display(generation);
        thread::sleep(Duration::from_secs(1));
        grid.step();
        generation += 1;
    }
}
