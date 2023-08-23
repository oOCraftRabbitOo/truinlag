use crate::runtime::manager;
use std::fs;
use truinlag::error::Result;
use truinlag::*;

fn handle_command(command: commands::Command, state: &mut GameState) -> commands::Response {
    use truinlag::commands::Command;
    match command {
        Command::Catch { catcher, caught } => todo!(),
        Command::Complete {
            completer,
            completed,
        } => todo!(),
        Command::End => todo!(),
        Command::MakeCatcher(team) => todo!(),
        Command::MakeRunner(team) => todo!(),
        Command::Start(config) => todo!(),
        Command::Status => todo!(),
        Command::Stop => todo!(),
    }
}

fn get_config(filename: &str) -> Result<Config> {
    Ok(ron::from_str(&fs::read_to_string(&filename)?)?)
}

fn get_players(filename: &str) -> Result<Vec<Player>> {
    Ok(ron::from_str(&fs::read_to_string(&filename)?)?)
}

fn generate_teams<'a>(filename: &str, players: &'a Vec<Player>) -> Result<Vec<Team<'a>>> {
    let raw_teams: Vec<RawTeam> = ron::from_str(&fs::read_to_string(&filename)?)?;
    Ok(raw_teams
        .iter()
        .map(|raw_team| {
            Team::new(
                raw_team.name.clone(),
                players
                    .iter()
                    .filter(|player| {
                        raw_team.players.iter().any(|raw_player| {
                            match players.iter().find(|player| player.name == *raw_player) {
                                Some(_) => true,
                                None => false,
                            }
                        })
                    })
                    .collect(),
            )
        })
        .collect())
    // TODO: return LoadError::PlayerNotFound when necessary
}

fn start_game() {
    let config_file = "config.ron";
    let config = get_config(config_file).unwrap_or_else(|error| {
        println!(
            "Something went wrong reading config file at {}, loading default:\n{}",
            config_file, error
        );
        default_config()
    });

    let player_file = "players.ron";
    let players = get_players(player_file).unwrap_or_else(|error| {
        panic!(
            "Something went wrong reading player file at {}:\n{}",
            player_file, error
        );
    });

    let team_file = "teams.ron";
    let teams = generate_teams(team_file, &players).unwrap_or_else(|error| {
        panic!(
            "Something went wrong generating teams from {}:\n{}",
            team_file, error
        )
    });
}

#[tokio::main]
async fn main() -> Result<()> {
    manager().await.unwrap();

    Ok(())
}
