//! The session a built scene plays: takes and rests, a pinned seed, the
//! weather story, and pausing.

use crate::config::scene::SceneConfig;

/// The meadow of rests birds builds: each bird kind reports its takes by
/// name, utters and rests by itself, and no two birds utter the same sequence.
/// Rendered without an audio device, at 48 kHz for two minutes.
#[test]
fn the_meadow_birds_sing_their_own_takes_with_rests() {
    let scene = SceneConfig::load("scenes/meadow-birds.yaml").expect("scene should load");
    let result = scene.build().expect("scene should build");
    let birds: Vec<_> = result
        .description
        .sources
        .iter()
        .filter(|source| source.takes.len() > 1)
        .map(|source| {
            (
                source.name.clone(),
                source.slot as usize,
                source.takes.len(),
            )
        })
        .collect();
    assert_eq!(
        birds
            .iter()
            .map(|bird| (bird.0.as_str(), bird.2))
            .collect::<Vec<_>>(),
        [
            ("Blackbird", 3),
            ("Robin", 5),
            ("Dunnock", 5),
            ("Blue tit", 6)
        ]
    );
    assert_eq!(result.description.sources[1].takes[0].name, "Phrase 1");
    assert!(
        result.description.sources[0].takes.is_empty(),
        "the wind has no takes"
    );

    // The session plans every bird ahead: two minutes of it, read without
    // playing, are what two minutes of playing would utter.
    let planned = result.planner.upcoming(120.0);
    let mut uttered: Vec<Vec<u8>> = vec![Vec::new(); birds.len()];
    let mut sounding = vec![0f64; birds.len()];
    for (slot, utterance) in &planned {
        if let Some(index) = birds.iter().position(|bird| bird.1 == *slot) {
            uttered[index].push(utterance.take);
            sounding[index] += utterance.seconds;
        }
    }
    assert!(
        planned
            .iter()
            .all(|(slot, _)| birds.iter().any(|bird| bird.1 == *slot)),
        "only voices with takes and rests are planned; the wind is not"
    );
    for (index, (name, _, takes)) in birds.iter().enumerate() {
        let uttered_by_one = &uttered[index];
        assert!(
            uttered_by_one.len() >= 8,
            "{name} sang only {} phrases in two minutes",
            uttered_by_one.len()
        );
        assert!(
            uttered_by_one.windows(2).all(|pair| pair[0] != pair[1]),
            "{name} repeated a phrase"
        );
        assert!(uttered_by_one.iter().all(|take| (*take as usize) < *takes));
        let resting = 1.0 - sounding[index] / 120.0;
        assert!(
            resting > 0.4,
            "{name} rested only {:.0} % of the time",
            resting * 100.0
        );
    }
    assert_ne!(
        uttered[1], uttered[2],
        "two birds with five takes must not fall into step"
    );
}

/// A take heard on its own: only that take, uttered at once and then
/// silence, whatever the sound's own rests are. A generator has no takes.
#[test]
fn one_take_is_heard_on_its_own_at_once() {
    let scene = SceneConfig::load("scenes/meadow-birds.yaml").expect("scene should load");
    let result = scene.clone().build().expect("scene should build");
    let (id, blackbird) = scene
        .sounds
        .iter()
        .find(|(id, _)| id.as_str() == "blackbird")
        .expect("a blackbird");
    let mut built = result
        .sounds
        .build_take(id, blackbird, 1, 15)
        .expect("its second take builds");
    assert_eq!(built.description.takes.len(), 1);
    assert_eq!(built.description.takes[0].name, "Phrase 2");
    assert_eq!(built.description.slot, 15);

    let rate = 48_000.0;
    built.source.next_sample(rate);
    assert_eq!(built.source.sounding_take(), Some(0), "it starts at once");
    let mut heard_for = 0usize;
    for _ in 0..(rate as usize * 20) {
        built.source.next_sample(rate);
        if built.source.sounding_take().is_some() {
            heard_for += 1;
        }
    }
    assert!(
        heard_for > 0 && heard_for < rate as usize * 20,
        "once, then silent"
    );

    let (wind_id, wind) = scene.sounds.iter().next().expect("the wind");
    assert!(result.sounds.build_take(wind_id, wind, 0, 15).is_err());
    assert!(result.sounds.build_take(id, blackbird, 9, 15).is_err());
}

/// A scene can pin its seed: then two builds plan the same session. Without
/// one, every play is a new session.
#[test]
fn a_seed_in_the_scene_plays_the_same_session_again() {
    let pinned = |seed: Option<u64>| {
        let mut scene = SceneConfig::load("scenes/meadow-birds.yaml").expect("scene should load");
        scene.seed = seed;
        let result = scene.build().expect("scene should build");
        (result.planner.seed(), result.planner.upcoming(60.0))
    };
    let (seed, first) = pinned(Some(7));
    assert_eq!(seed, 7);
    let (_, again) = pinned(Some(7));
    let takes = |plan: &[(usize, crate::plan::Planned)]| {
        plan.iter()
            .map(|(slot, planned)| (*slot, planned.take))
            .collect::<Vec<_>>()
    };
    assert_eq!(takes(&first), takes(&again));
    let (_, other) = pinned(Some(8));
    assert_ne!(takes(&first), takes(&other));
    let (fresh, _) = pinned(None);
    let (fresh_again, _) = pinned(None);
    assert_ne!(fresh, fresh_again, "without a seed each play is new");
}

/// The passing storm builds, and its plan strikes thunder only in a storm,
/// with the birds quiet in the downpour: over three hours of plan there is
/// thunder, and none of it in the first dry minute.
#[test]
fn the_passing_storm_strikes_thunder_only_in_its_storms() {
    let mut scene = SceneConfig::load("scenes/passing-storm.yaml").expect("scene should load");
    scene.seed = Some(3);
    let result = scene.build().expect("scene should build");
    let thunder = result
        .description
        .sources
        .iter()
        .find(|source| source.id == "thunder")
        .expect("thunder")
        .slot as usize;
    let weather = result
        .planner
        .weather()
        .expect("a nature scene has weather");
    let claps: Vec<_> = result
        .planner
        .upcoming(3.0 * 3600.0)
        .into_iter()
        .filter(|(slot, _)| *slot == thunder)
        .collect();
    assert!(
        claps.len() > 10,
        "only {} claps in three hours",
        claps.len()
    );
    for (_, clap) in &claps {
        let storm = weather.at(clap.at).get(crate::plan::Condition::Storm);
        assert!(storm > 0.0, "a clap at {:.0} s with no storm", clap.at);
    }
}

/// Under the passing storm's weather, as the plan decides it: the birds stop
/// as soon as it really rains, the frog croaks in light rain but not in the
/// downpour or a gale, and the storm wind is silent until a storm comes.
#[test]
fn in_the_passing_storm_each_creature_keeps_to_its_weather() {
    use crate::plan::Condition;
    let mut scene = SceneConfig::load("scenes/passing-storm.yaml").expect("scene should load");
    scene.seed = Some(3);
    let result = scene.build().expect("scene should build");
    let weather = result
        .planner
        .weather()
        .expect("a nature scene has weather");
    let slot = |id: &str| {
        result
            .description
            .sources
            .iter()
            .find(|source| source.id == id)
            .expect(id)
            .slot as usize
    };
    let planned = result.planner.upcoming(3.0 * 3600.0);
    let under = |id: &str, condition: Condition| -> Vec<f32> {
        planned
            .iter()
            .filter(|(own, _)| *own == slot(id))
            .map(|(_, utterance)| weather.at(utterance.at).get(condition))
            .collect()
    };
    for bird in ["robin", "blackbird"] {
        let rain = under(bird, Condition::Rain);
        assert!(rain.len() > 100, "{bird} hardly sang");
        assert!(
            rain.iter().all(|rain| *rain < 0.35),
            "{bird} sang in real rain"
        );
    }
    let frog_rain = under("frog", Condition::Rain);
    let frog_wind = under("frog", Condition::Wind);
    assert!(
        frog_rain.iter().any(|rain| *rain > 0.1),
        "the frog never croaked in light rain"
    );
    assert!(
        frog_rain.iter().all(|rain| *rain < 0.85),
        "the frog croaked in the downpour"
    );
    assert!(
        frog_wind.iter().all(|wind| *wind < 0.75),
        "the frog croaked in a gale"
    );
}

/// Pausing fades the sound out, then stops the session's clock, so the plan
/// and the weather hold where they are; resuming fades back in from there.
#[test]
fn a_paused_scene_is_silent_and_its_clock_stands_still() {
    let mut result = SceneConfig::load("scenes/frog.yaml")
        .expect("scene should load")
        .build()
        .expect("scene should build");
    let scene = &mut result.scene;
    scene.sample_rate = 48_000.0;
    let channels = scene.speaker_layout.total_channels();
    let mut block = vec![0.0f32; 512 * channels];
    for _ in 0..20 {
        scene.render(&mut block, channels);
    }
    scene.paused = true;
    // 50 ms of fade is under five blocks of 512 at 48 kHz.
    for _ in 0..6 {
        scene.render(&mut block, channels);
    }
    let stopped_at = scene.clock.samples();
    for _ in 0..20 {
        scene.render(&mut block, channels);
        assert!(
            block.iter().all(|sample| *sample == 0.0),
            "silent while paused"
        );
    }
    assert_eq!(scene.clock.samples(), stopped_at, "the clock stood still");
    scene.paused = false;
    scene.render(&mut block, channels);
    assert_eq!(
        scene.clock.samples(),
        stopped_at + 512,
        "and runs on from where it stopped"
    );
}

/// A scene's weather says where the wind comes from, as a compass bearing.
/// Left out it is the south-west and is not written back; set, it is kept.
#[test]
fn a_scenes_weather_names_where_the_wind_comes_from() {
    use crate::config::WeatherConfig;
    let plain: WeatherConfig = serde_yaml::from_str("story: held\nwind: 0.3\n").expect("parses");
    assert_eq!(plain.wind_from, crate::plan::PREVAILING_WIND_FROM);
    assert_eq!(plain.story(1).expect("a story").wind_from(), 225.0);
    assert!(!serde_yaml::to_string(&plain)
        .expect("writes")
        .contains("wind_from"));

    let westerly: WeatherConfig =
        serde_yaml::from_str("story: held\nwind: 0.3\nwind_from: 270\n").expect("parses");
    assert_eq!(westerly.story(1).expect("a story").wind_from(), 270.0);
    let written = serde_yaml::to_string(&westerly).expect("writes");
    assert!(written.contains("wind_from: 270"), "{written}");
    assert_eq!(
        serde_yaml::from_str::<WeatherConfig>(&written).expect("reads back"),
        westerly
    );
}
