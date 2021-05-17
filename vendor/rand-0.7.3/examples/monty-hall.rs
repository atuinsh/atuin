// Copyright 2018 Developers of the Rand project.
// Copyright 2013-2018 The Rust Project Developers.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! ## Monty Hall Problem
//!
//! This is a simulation of the [Monty Hall Problem][]:
//!
//! > Suppose you're on a game show, and you're given the choice of three doors:
//! > Behind one door is a car; behind the others, goats. You pick a door, say
//! > No. 1, and the host, who knows what's behind the doors, opens another
//! > door, say No. 3, which has a goat. He then says to you, "Do you want to
//! > pick door No. 2?" Is it to your advantage to switch your choice?
//!
//! The rather unintuitive answer is that you will have a 2/3 chance of winning
//! if you switch and a 1/3 chance of winning if you don't, so it's better to
//! switch.
//!
//! This program will simulate the game show and with large enough simulation
//! steps it will indeed confirm that it is better to switch.
//!
//! [Monty Hall Problem]: https://en.wikipedia.org/wiki/Monty_Hall_problem

#![cfg(feature = "std")]

use rand::distributions::{Distribution, Uniform};
use rand::Rng;

struct SimulationResult {
    win: bool,
    switch: bool,
}

// Run a single simulation of the Monty Hall problem.
fn simulate<R: Rng>(random_door: &Uniform<u32>, rng: &mut R) -> SimulationResult {
    let car = random_door.sample(rng);

    // This is our initial choice
    let mut choice = random_door.sample(rng);

    // The game host opens a door
    let open = game_host_open(car, choice, rng);

    // Shall we switch?
    let switch = rng.gen();
    if switch {
        choice = switch_door(choice, open);
    }

    SimulationResult {
        win: choice == car,
        switch,
    }
}

// Returns the door the game host opens given our choice and knowledge of
// where the car is. The game host will never open the door with the car.
fn game_host_open<R: Rng>(car: u32, choice: u32, rng: &mut R) -> u32 {
    use rand::seq::SliceRandom;
    *free_doors(&[car, choice]).choose(rng).unwrap()
}

// Returns the door we switch to, given our current choice and
// the open door. There will only be one valid door.
fn switch_door(choice: u32, open: u32) -> u32 {
    free_doors(&[choice, open])[0]
}

fn free_doors(blocked: &[u32]) -> Vec<u32> {
    (0..3).filter(|x| !blocked.contains(x)).collect()
}

fn main() {
    // The estimation will be more accurate with more simulations
    let num_simulations = 10000;

    let mut rng = rand::thread_rng();
    let random_door = Uniform::new(0u32, 3);

    let (mut switch_wins, mut switch_losses) = (0, 0);
    let (mut keep_wins, mut keep_losses) = (0, 0);

    println!("Running {} simulations...", num_simulations);
    for _ in 0..num_simulations {
        let result = simulate(&random_door, &mut rng);

        match (result.win, result.switch) {
            (true, true) => switch_wins += 1,
            (true, false) => keep_wins += 1,
            (false, true) => switch_losses += 1,
            (false, false) => keep_losses += 1,
        }
    }

    let total_switches = switch_wins + switch_losses;
    let total_keeps = keep_wins + keep_losses;

    println!(
        "Switched door {} times with {} wins and {} losses",
        total_switches, switch_wins, switch_losses
    );

    println!(
        "Kept our choice {} times with {} wins and {} losses",
        total_keeps, keep_wins, keep_losses
    );

    // With a large number of simulations, the values should converge to
    // 0.667 and 0.333 respectively.
    println!(
        "Estimated chance to win if we switch: {}",
        switch_wins as f32 / total_switches as f32
    );
    println!(
        "Estimated chance to win if we don't: {}",
        keep_wins as f32 / total_keeps as f32
    );
}
