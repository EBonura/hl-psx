use std::env;
use std::error::Error;
use std::path::Path;

mod audio;
mod generators;
mod menu;
mod sprites;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn usage() -> ! {
    eprintln!(
        "usage:\n\
         \x20 hl-content menu <valve-dir> <out-dir> [font.ttf]\n\
         \x20 hl-content music <valve-dir> <out-dir>\n\
         \x20 hl-content sfx <sound-dir> <out.psxa> <psxed>\n\
         \x20 hl-content voices <valve-dir> <map-list> <out-dir> <psxed>\n\
         \x20 hl-content sprites <valve-dir> <map-list> <out-dir>\n\
         \x20 hl-content merge-model <geometry.psxm> <texture.psxm>\n\
         \x20 hl-content clips <roster.txt> <clips.txt>\n\
         \x20 hl-content studio-events <models-dir> <roster.txt> <out.txt>\n\
         \x20 hl-content transition-props <maps-dir> <out.txt> <maps...>"
    );
    std::process::exit(2);
}

fn arg<'a>(args: &'a [String], index: usize) -> &'a str {
    args.get(index)
        .map(String::as_str)
        .unwrap_or_else(|| usage())
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    match arg(&args, 1) {
        "menu" => menu::build(
            Path::new(arg(&args, 2)),
            Path::new(arg(&args, 3)),
            args.get(4).map(Path::new),
        ),
        "music" => audio::build_music(Path::new(arg(&args, 2)), Path::new(arg(&args, 3))),
        "sfx" => audio::build_sfx(
            Path::new(arg(&args, 2)),
            Path::new(arg(&args, 3)),
            Path::new(arg(&args, 4)),
        ),
        "voices" => audio::build_voices(
            Path::new(arg(&args, 2)),
            arg(&args, 3),
            Path::new(arg(&args, 4)),
            Path::new(arg(&args, 5)),
        ),
        "sprites" => sprites::build(
            Path::new(arg(&args, 2)),
            arg(&args, 3),
            Path::new(arg(&args, 4)),
        ),
        "merge-model" => {
            generators::merge_model(Path::new(arg(&args, 2)), Path::new(arg(&args, 3)))
        }
        "clips" => generators::clips(Path::new(arg(&args, 2)), Path::new(arg(&args, 3))),
        "studio-events" => generators::studio_events(
            Path::new(arg(&args, 2)),
            Path::new(arg(&args, 3)),
            Path::new(arg(&args, 4)),
        ),
        "transition-props" => generators::transition_props(
            Path::new(arg(&args, 2)),
            Path::new(arg(&args, 3)),
            &args[4..],
        ),
        _ => usage(),
    }
}
