use core::{mem::transmute, ptr::addr_of_mut};

use alloc::vec;
use alloc::vec::Vec;

use crate::{
    apu::{self, Sfx},
    constants::{AT_SPRITE, DT, GRID_SIZE, HEART_SPRITE, HEIGHT, ORIGIN, PLAYER_WIDTH, ROW, WIDTH},
    io,
    level::{draw_level, get_tile_at, make_level, map_pos_to_tile_index, Tile},
    ppu,
    ppu_buffer::{self, BufferDirective},
    rng::Rng,
    sprites::{self, SpriteState},
    utils::{self, debug_value, inc_u8, Addr, DPos, Orientation, Pos, Sign, Vec2},
};

// called before enabling nmi
pub fn init(game: &mut Game) {
    // palettes
    ppu::write_bytes(ppu::PAL_BG_0, &[0x0E, 0x30, 0x12, 0x26]);
    ppu::write_bytes(ppu::PAL_SPRITE_0 + 3, &[0x15]);

    draw_level(&mut game.tiles);

    // text
    ppu::draw_ascii(ORIGIN + 0x06, "HEART-MAN");
}

pub fn frame(game: &mut Game, apu: &mut apu::APU, sprites: &mut SpriteState) {
    game.step(apu);

    sprites.add(&game.player.pos, HEART_SPRITE, 0);

    for meanie in game.meanies.iter() {
        sprites.add(&meanie.pos, AT_SPRITE, 0)
    }
}

struct Player {
    pos: Pos,
}

struct Meanie {
    pos: Pos,
    vel: DPos,
    orientation: Orientation,
    n_turns: u8,
}

type Collision = Vec2<Option<Sign>>;

pub struct Game {
    player: Player,
    tiles: [Tile; GRID_SIZE as usize],
    grabbed_coin_index: Option<u16>,
    n_coins: u8,
    meanies: Vec<Meanie>,
    rng: Rng,
}

impl Game {
    pub fn new(some_game: &mut Option<Game>) {
        *some_game = Some(Self {
            rng: Rng::new(None),
            player: Player {
                pos: Pos {
                    x: WIDTH / 2,
                    y: HEIGHT - 10,
                },
            },
            tiles: [Tile::Nothing; GRID_SIZE as usize],
            grabbed_coin_index: None,
            n_coins: 0,
            meanies: vec![
                Meanie {
                    pos: Pos {
                        x: WIDTH / 2 + 16,
                        y: HEIGHT - 20,
                    },
                    vel: DPos::new(-1, 0),
                    orientation: Orientation::Widdershins,
                    n_turns: 0,
                },
                Meanie {
                    pos: Pos {
                        x: WIDTH / 3,
                        y: HEIGHT - 20,
                    },
                    vel: DPos::new(0, 1),
                    orientation: Orientation::Widdershins,
                    n_turns: 0,
                },
                Meanie {
                    pos: Pos {
                        x: WIDTH / 3,
                        y: HEIGHT / 2 - 16,
                    },
                    vel: DPos::new(0, -1),
                    orientation: Orientation::Widdershins,
                    n_turns: 0,
                },
            ],
        });
        let game = some_game.as_mut().unwrap();
        make_level(&mut game.tiles, &mut game.rng);

        game.n_coins = game
            .tiles
            .iter()
            .map(|t| match t {
                Tile::Coin => 1,
                _ => 0,
            })
            .sum();
    }

    fn step(&mut self, apu: &mut apu::APU) {
        let mut player_delta = player_movement_delta(io::controller_buttons(), &self.player.pos);

        let collision = check_box_collision(
            &self.tiles,
            Tile::Wall,
            PLAYER_WIDTH as i8,
            &self.player.pos,
            &player_delta,
        );
        if let Some(_) = collision.x {
            player_delta.x = 0;
            if !apu.is_playing() {
                apu.play_sfx(Sfx::Lock);
            }
        }
        if let Some(_) = collision.y {
            player_delta.y = 0;
            if !apu.is_playing() {
                apu.play_sfx(Sfx::Lock);
            }
        }

        let player_center = self.player.pos.shifted(&DPos::new(4, 4));
        if let Tile::Coin = get_tile_at(&self.tiles, &player_center) {
            let index = map_pos_to_tile_index(&player_center);
            self.tiles[index as usize] = Tile::Nothing;
            self.grabbed_coin_index = Some(index);
            self.n_coins -= 1;
            apu.play_sfx(Sfx::LevelUp);
        }

        self.player.pos.inc(&player_delta);

        for meanie in self.meanies.iter_mut() {
            update_meanie(&self.tiles, meanie)
        }

        self.draw()
    }

    fn draw(&mut self) {
        ppu_buffer::clear();
        ppu_buffer::push(BufferDirective::Index(ORIGIN));
        ppu_buffer::extend(draw_digits(self.n_coins));

        if let Some(index) = self.grabbed_coin_index {
            ppu_buffer::push(BufferDirective::Index(ORIGIN + index));
            ppu_buffer::push(BufferDirective::Tile(HEART_SPRITE));
            self.grabbed_coin_index = None;
        }
    }
}

fn draw_digits(x: u8) -> Vec<BufferDirective> {
    let mut digits = [0; 3];
    for (x, y) in digits
        .iter_mut()
        .rev()
        .zip(utils::u8_to_decimal(x).into_iter())
    {
        *x = y
    }
    digits
        .map(|d| BufferDirective::Tile(io::digit_to_ascii(d) - 32))
        .into_iter()
        .collect()
}

fn player_movement_delta(buttons: u8, player_pos: &Pos) -> DPos {
    let mut delta = DPos::zero();

    if buttons & io::LEFT != 0 && player_pos.x > 0 {
        delta.x = -2;
    }
    if buttons & io::RIGHT != 0 && player_pos.x + 8 < WIDTH {
        delta.x = 2;
    }
    if buttons & io::UP != 0 && player_pos.y > 0 {
        delta.y = -2;
    }
    if buttons & io::DOWN != 0 && player_pos.y + 8 < WIDTH {
        delta.y = 2;
    }

    delta
}

fn i8_to_sign(i: i8) -> Option<Sign> {
    if i > 0 {
        Some(Sign::Plus)
    } else if i < 0 {
        Some(Sign::Minus)
    } else {
        None
    }
}

fn check_box_collision(
    tiles: &[Tile],
    colliding_tile: Tile,
    width: i8,
    pos: &Pos,
    pos_delta: &DPos,
) -> Collision {
    let mut collision = Collision { x: None, y: None };
    for box_delta in [
        DPos::new(0, 0),
        DPos::new(0, width),
        DPos::new(width, 0),
        DPos::new(width, width),
    ] {
        let box_pos = pos.shifted(&box_delta);
        if get_tile_at(tiles, &box_pos.shifted(&pos_delta.x_vec())) == colliding_tile {
            collision.x = i8_to_sign(pos_delta.x);
        }
        if get_tile_at(tiles, &box_pos.shifted(&pos_delta.y_vec())) == colliding_tile {
            collision.y = i8_to_sign(pos_delta.y);
        }
    }
    collision
}

fn update_meanie(tiles: &[Tile], meanie: &mut Meanie) {
    let mut delta = DPos::zero();
    for _ in 0..3 {
        // stop trying after 3 attempts in case we're stuck
        delta = meanie.vel.scaled(DT as i8);
        let collision =
            check_box_collision(tiles, Tile::Wall, PLAYER_WIDTH as i8, &meanie.pos, &delta);
        if let Vec2 { x: None, y: None } = collision {
            break;
        }
        delta = delta.rotate(meanie.orientation);
        meanie.vel = delta;
        meanie.n_turns += 1;
    }

    meanie.pos.inc(&delta);

    if meanie.n_turns > 50 {
        meanie.orientation = meanie.orientation.reverse();
        meanie.n_turns = 0
    }
}
