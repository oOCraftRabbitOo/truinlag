pub mod api;
pub mod commands;

/*
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Config {
    link_kaffskala: String,
    link_custom_challenges: String,
    relative_standard_deviation: f64,
    specific_zone_chance: f64,
    points_per_kaffskala: i32,
    points_per_güteklasse: i32,
    zones: Vec<i32>,
    s_bahn_zones: Vec<i32>,
}

impl Config {
    pub fn new() -> Config {
        Config {
        link_kaffskala: String::from("https://docs.google.com/spreadsheets/d/13DlG2BSQfolPCsoj2LBREeIUgThji_zgeE_q-gHQSL4/export?format=csv"),
        link_custom_challenges: String::from("https://docs.google.com/spreadsheets/d/10EaV2iZUAP8oZH7PLdoWGx9lQPvvN3Hfu7jH2zqKPv4/export?format=csv"),
        relative_standard_deviation: 0.1,
        specific_zone_chance: 0.25,
        points_per_kaffskala: 0,
        points_per_güteklasse: 0,
        zones: vec![116, 115, 161, 162, 113, 114, 124, 160, 163, 118, 117, 112, 123, 120, 164, 111, 121, 122, 170, 171, 154, 110, 173, 172, 135, 131, 130, 155, 150, 156, 151, 152, 153, 181, 180, 133, 143, 142, 141, 140, 130, 132, 134],
        s_bahn_zones: vec![132, 110, 151, 180, 120, 181, 155, 133, 156, 117, 121, 141, 142, 134, 112, 154],
    }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Player {
    pub name: String,
    pub nicknames: Vec<String>,
    pub discord: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RawTeam {
    pub name: String,
    pub players: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TeamOwningPlayers {
    pub name: String,
    pub players: Vec<Player>,
    pub is_catcher: bool,
    pub points_since_catcher: i32,
    pub completed_challenges: Vec<Challenge>,
    pub active_challenges: Vec<Challenge>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Team<'a> {
    pub name: String,
    pub players: Vec<&'a Player>,
    pub is_catcher: bool,
    pub points_since_catcher: i32,
    pub completed_challenges: Vec<Challenge>,
    pub active_challenges: Vec<Challenge>,
}

impl Team<'_> {
    pub fn new(name: String, players: Vec<&Player>) -> Team {
        Team {
            name,
            players,
            is_catcher: false,
            points_since_catcher: 0,
            completed_challenges: Vec::new(),
            active_challenges: Vec::new(),
        }
    }

    pub fn owning(&self) -> TeamOwningPlayers {
        TeamOwningPlayers {
            name: self.name.clone(),
            is_catcher: self.is_catcher,
            points_since_catcher: self.points_since_catcher,
            completed_challenges: self.completed_challenges.clone(),
            active_challenges: self.active_challenges.clone(),
            players: self.players.iter().map(|&player| player.clone()).collect(),
        }
    }

    pub fn points(&self) -> i32 {
        self.completed_challenges.iter().map(|x| x.points).sum()
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LocalChallenge {
    Custom(CustomLocalChallenge),
    Generated(GeneratedLocalChallenge),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CustomLocalChallenge {
    name: String,
    description: String,
    points: i32,
    fixed: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GeneratedLocalChallenge {
    municipality: String,
    kaffskala: i32,
    güteklasse: i32,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GlobalChallenge {
    name: String,
    description: String,
    points: i32,
    min_r: i32,
    max_r: i32,
    ppr: i32,
    zoneable: bool,
    fixed_points: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RawChallenge {
    Local(LocalChallenge),
    Global(GlobalChallenge),
}

impl RawChallenge {
    pub fn generate_challenge(&self, config: &Config) -> Challenge {
        // function for the random point adjustment that happens for every type of challenge
        fn adjust(num: i32, config: &Config) -> i32 {
            use rand_distr::{Distribution, Normal};

            let std_dev: f64 = num as f64 * config.relative_standard_deviation;
            let normal = Normal::new(num as f64, std_dev).unwrap_or_else(|error| {
                panic!(
                    "An error occurred while generating the random distribution:\n{}",
                    error
                );
            });
            normal.sample(&mut thread_rng()) as i32
        }
        match self {
            RawChallenge::Local(local) => match local {
                // Generating challenges for local, custom raw challenges
                LocalChallenge::Custom(custom) => {
                    let points = if custom.fixed {
                        custom.points
                    } else {
                        adjust(custom.points, config)
                    };
                    Challenge {
                        title: custom.name.clone(),
                        description: format!("{} *{} Pünkt*", custom.description, points),
                        source: RawChallenge::Local(LocalChallenge::Custom(custom.clone())),
                        points,
                    }
                }

                // Generating challenges for the type of local challenge which is generated on the basis of kaffskala and güteklasse
                LocalChallenge::Generated(generated) => {
                    let points = adjust(
                        config.points_per_kaffskala * generated.kaffskala
                            + config.points_per_güteklasse * generated.güteklasse,
                        config,
                    );
                    Challenge {
                        title: format!("Usflug uf {}", generated.municipality),
                        description: format!(
                            "Gönd nach {}. *{} Pünkt*",
                            generated.municipality, points
                        ),
                        source: RawChallenge::Local(LocalChallenge::Generated(generated.clone())),
                        points,
                    }
                }
            },

            // Generating challenges for global challenges
            RawChallenge::Global(global) => {
                let zone = config
                    .zones
                    .choose(&mut thread_rng())
                    .expect("Problem choosing a random zone, list of zones might be empty");
                let s_bahn_zone = config.s_bahn_zones.choose(&mut thread_rng()).expect(
                    "Problem choosing a random s-bahn zone, list of s-bahn zones might be empty",
                );
                let random_repetitions = if global.min_r >= global.max_r {
                    0
                } else {
                    thread_rng().gen_range(global.min_r..=global.max_r)
                };
                let mut description = global.description.clone();
                let mut points = global.points + random_repetitions;
                if global.zoneable && thread_rng().gen_bool(config.specific_zone_chance) {
                    points *= 2;
                    description = format!(
                        "{} Damit ihr Pünkt überchömed, mached das i de Zone {}.",
                        description, zone
                    )
                }
                description = description
                    .replace("%r", &random_repetitions.to_string())
                    .replace("%z", &zone.to_string())
                    .replace("%s", &s_bahn_zone.to_string());

                description = format!("{} *{} Pünkt*", description, points);

                points = adjust(points, config);

                Challenge {
                    title: global.name.clone(),
                    source: RawChallenge::Global(global.clone()),
                    description,
                    points,
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Challenge {
    title: String,
    description: String,
    points: i32,
    source: RawChallenge,
}

impl fmt::Display for Challenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "**{}**\n{}", self.title, self.description)
    }
}

pub struct GameState<'a> {
    pub config: Config,
    pub running: bool,
    pub challenges: Vec<RawChallenge>,
    pub teams: Vec<Team<'a>>,
    pub players: Vec<Player>,
}

pub fn get_challenges(config: &Config) -> Result<Vec<RawChallenge>> {
    let mut challenges: Vec<RawChallenge> = Vec::new();

    //Kaffskala challenges generiere und de liste appende
    #[derive(Debug, serde::Deserialize)]
    #[allow(non_snake_case)]
    struct GeneratedLocalChallengeData {
        Ort: String,
        Kaffskala: i32,
        Güteklasse: i32,
    }

    let mut rdr = csv::Reader::from_reader(reqwest::blocking::get(&config.link_kaffskala)?);
    for result in rdr.deserialize() {
        let challenge_data: GeneratedLocalChallengeData = match result {
            Ok(result) => result,
            Err(error) => {
                eprintln!(
                    "Error occurred in gathering the Kaffskala data:\n{}\nIgnoring and continuing",
                    error
                );
                continue;
            }
        };
        challenges.push(RawChallenge::Local(LocalChallenge::Generated(
            GeneratedLocalChallenge {
                municipality: challenge_data.Ort,
                kaffskala: challenge_data.Kaffskala,
                güteklasse: challenge_data.Güteklasse,
            },
        )))
    }

    //custom challenges generiere und appende
    #[derive(Debug, serde::Deserialize)]
    struct CustomChallengeData {
        title: String,
        description: String,
        min: i32,
        max: i32,
        points: i32,
        ppr: i32,
        zoneable: i32,
        fixed: i32,
        spezifisch: i32,
    }

    fn infer_bool(num: i32) -> bool {
        if num == 0 {
            false
        } else {
            true
        }
    }

    let mut rdr = csv::Reader::from_reader(reqwest::blocking::get(&config.link_custom_challenges)?);
    for result in rdr.deserialize() {
        let challenge_data: CustomChallengeData = match result {
            Ok(result) => result,
            Err(error) => {
                eprintln!(
                    "Error occured in gathering custom challenges:\n{}\nIgnoring and continuing",
                    error
                );
                continue;
            }
        };
        if !infer_bool(challenge_data.spezifisch) {
            challenges.push(RawChallenge::Global(GlobalChallenge {
                name: challenge_data.title,
                description: challenge_data.description,
                points: challenge_data.points,
                min_r: challenge_data.min,
                max_r: challenge_data.max,
                ppr: challenge_data.ppr,
                zoneable: infer_bool(challenge_data.zoneable),
                fixed_points: infer_bool(challenge_data.fixed),
            }))
        } else {
            challenges.push(RawChallenge::Local(LocalChallenge::Custom(
                CustomLocalChallenge {
                    name: challenge_data.title,
                    description: challenge_data.description,
                    points: challenge_data.points,
                    fixed: infer_bool(challenge_data.fixed),
                },
            )))
        }
    }

    Ok(challenges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_challenges() {
        let challenges = get_challenges(&Config::new()).unwrap_or_else(|error| {
            panic!("The get_challenges function returned an error:\n{}", error);
        });
        for i in &challenges {
            println!("{:?}", i)
        }
        assert!(
            challenges.contains(&RawChallenge::Local(LocalChallenge::Custom(
                CustomLocalChallenge {
                    name: "Absoluter Rheinfall".to_owned(),
                    description: "Gönd zum Rheinfall.".to_owned(),
                    points: 400,
                    fixed: false,
                }
            )))
        );
        assert!(challenges.contains(&RawChallenge::Global(GlobalChallenge {
            name: "Eflizzer".to_owned(),
            description: "Fahred min. 1 Station mit em Schnellzug.".to_owned(),
            points: 250,
            min_r: 0,
            max_r: 0,
            ppr: 0,
            zoneable: false,
            fixed_points: false
        })));
        assert!(challenges.contains(&RawChallenge::Global(GlobalChallenge {
            name: "Bezirksdrifter II".to_owned(),
            description: "Gönd a %r Bezirkshauptört.".to_owned(),
            points: 50,
            min_r: 2,
            max_r: 4,
            ppr: 200,
            zoneable: false,
            fixed_points: false
        })));
        assert!(
            challenges.contains(&RawChallenge::Local(LocalChallenge::Generated(
                GeneratedLocalChallenge {
                    municipality: "Wernetshausen".to_owned(),
                    kaffskala: 6,
                    güteklasse: 5,
                }
            )))
        )
    }

    #[test]
    fn generate_challenges() {
        let challenges = get_challenges(&Config::new()).unwrap_or_else(|error| {
            println!("The get_challenges function returned an error:\n{}", error);
            std::process::exit(1);
        });
        println!("{:?}", challenges);
        for challenge in challenges {
            println!("{}\n\n", challenge.generate_challenge(&Config::new()));
        }
    }

    #[tokio::test]
    async fn unix_stream_tomfoolery() {
        use bytes::Bytes;
        use futures::prelude::*;
        use futures::SinkExt;
        use tokio::net::UnixStream;
        use tokio_util::codec::{Framed, FramedRead, FramedWrite, LengthDelimitedCodec};
        let (stream1, stream2) = UnixStream::pair().unwrap();
        let mut transport1 = Framed::new(stream1, LengthDelimitedCodec::new());
        let mut transport2 = Framed::new(stream2, LengthDelimitedCodec::new());
        let frame = Bytes::from("hi");
        transport1.send(frame).await.unwrap();
        transport1.send(Bytes::from("lol")).await.unwrap();
        let response = transport2.next().await.unwrap().unwrap();
        println!("{:?}", response);
        transport2.send(Bytes::from("xD")).await.unwrap();
        let response = transport1.next().await.unwrap().unwrap();
        println!("{:?}", response);
        let response = transport2.next().await.unwrap().unwrap();
        println!("{:?}", response);

        use bincode;
        use commands::EngineAction as Command;
        let message = Command::Status;
        let frame = Bytes::from(bincode::serialize(&message).unwrap());
        transport1.send(frame).await.unwrap();
        let response = transport2.next().await.unwrap().unwrap();
        let response: Command = bincode::deserialize(&response).unwrap();
        println!("{:?}", response);

        let message = Command::Catch {
            catcher: String::from("hans"),
            caught: String::from("ueli"),
        };
        let frame = Bytes::from(bincode::serialize(&message).unwrap());
        transport1.send(frame).await.unwrap();
        let response = transport2.next().await.unwrap().unwrap();
        let response: Command = bincode::deserialize(&response).unwrap();
        println!("{:?}", response);

        let stream2: UnixStream = transport2.into_inner();
        let (read2, write2) = stream2.into_split();
        let mut transport_read = FramedRead::new(read2, LengthDelimitedCodec::new());
        let mut transport_write = FramedWrite::new(write2, LengthDelimitedCodec::new());
        transport_write.send(Bytes::from("hui")).await.unwrap();
        let response = transport1.next().await.unwrap().unwrap();
        println!("{:?}", response);
        transport1.send(Bytes::from("bubulu")).await.unwrap();
        let response = transport_read.next().await.unwrap().unwrap();
        println!("{:?}", response);
    }
}
*/
