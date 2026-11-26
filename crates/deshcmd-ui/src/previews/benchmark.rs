use super::*;
use crate::{
    Action, App, Command, Side,
    tests::{TestDesktop, settle},
};
use eframe::App as _;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires six local JPEG fixtures and a release build"]
fn quick_view_prefetch_latency() {
    let directory = PathBuf::from(std::env::var_os("DESHCMD_PREVIEW_BENCH_DIR").unwrap());
    let mut files: Vec<_> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jpg"))
        .collect();
    files.sort();
    assert!(files.len() > LOOKAHEAD + 1);
    #[derive(Default)]
    struct Measurements {
        samples: Vec<f64>,
        cancellation_ms: f64,
        hits: usize,
        peak: usize,
    }

    for scenario in ["open_after_pause", "sequential", "rapid_stop", "jump"] {
        let mut measurements = [Measurements::default(), Measurements::default()];
        let (a, b, c, d) = (&files[0], &files[1], &files[2], &files[3]);
        for trial in 0..5 {
            // Alternate order to reduce the effect of changes in system load.
            for prefetch in [trial % 2 != 0, trial % 2 == 0] {
                let Measurements {
                    samples,
                    cancellation_ms,
                    hits,
                    peak,
                } = &mut measurements[usize::from(prefetch)];
                let ctx = egui::Context::default();
                let creation = eframe::CreationContext::_new_kittest(ctx.clone());
                let mut app = App::new(
                    &creation,
                    Box::new(TestDesktop::default()),
                    directory.clone(),
                    [directory.clone(), directory.clone()],
                    true,
                );
                let (send, starts) = mpsc::channel();
                let (finish, finishes) = mpsc::channel();
                app.previews = Previews::with_decoder(
                    ctx.clone(),
                    Arc::new(move |key, kind, _, cancel| {
                        let _ = send.send(key.path.clone());
                        let preview =
                            engine::preview_cancellable(&key.path, kind, key.pixels, cancel);
                        let _ = finish.send((key.path.clone(), Instant::now()));
                        preview
                    }),
                );
                app.previews.prefetch = prefetch;
                settle(&mut app);
                if scenario != "open_after_pause" {
                    app.apply(Action::ToggleQuick(Side::Left), &ctx);
                }
                let mut frame = eframe::Frame::_new_kittest();
                let mut draw = |app: &mut App, target: &PathBuf| {
                    let output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                vec2(1280., 800.),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            app.logic(&ctx, &mut frame);
                            app.ui(ui, &mut frame);
                        },
                    );
                    *peak = (*peak).max(app.previews.warm_bytes);
                    let Some(entry) = app
                        .engine
                        .tab(Side::Left)
                        .current()
                        .filter(|e| e.path == *target)
                    else {
                        return false;
                    };
                    let Some(CachedPreview::Image(handle, _)) =
                        app.previews.get(entry, QUICK_PIXELS)
                    else {
                        return false;
                    };
                    ctx.tessellate(output.shapes, output.pixels_per_point).iter().any(|shape| {
                        matches!(&shape.primitive, egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == handle.id())
                    })
                };
                let select = |app: &mut App, path: &PathBuf| {
                    app.engine.dispatch(Command::Select(
                        Side::Left,
                        path.clone(),
                        engine::Select::Focus,
                    ));
                };
                select(&mut app, a);
                if scenario != "open_after_pause" {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while !draw(&mut app, a) {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                if scenario != "jump" {
                    let until = Instant::now() + Duration::from_secs(1);
                    while Instant::now() < until {
                        draw(&mut app, a);
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
                let target = match scenario {
                    "open_after_pause" => a,
                    "sequential" => b,
                    "rapid_stop" => {
                        for path in [b, c] {
                            select(&mut app, path);
                            draw(&mut app, path);
                            std::thread::sleep(Duration::from_millis(30));
                        }
                        d
                    }
                    _ => {
                        if prefetch {
                            let deadline = Instant::now() + Duration::from_secs(3);
                            while !starts.try_iter().any(|path| path == *b) {
                                assert!(Instant::now() < deadline);
                                draw(&mut app, a);
                                std::thread::sleep(Duration::from_millis(1));
                            }
                        }
                        files.last().unwrap()
                    }
                };
                *hits += usize::from(
                    app.previews
                        .warm_cache
                        .iter()
                        .any(|(key, _)| key.path == *target)
                        || app.previews.cached.keys().any(|key| key.path == *target),
                );
                let start = Instant::now();
                if scenario == "open_after_pause" {
                    app.apply(Action::ToggleQuick(Side::Left), &ctx);
                } else {
                    select(&mut app, target);
                }
                while !draw(&mut app, target) {
                    assert!(start.elapsed() < Duration::from_secs(5));
                    std::thread::sleep(Duration::from_millis(1));
                }
                samples.push(start.elapsed().as_secs_f64() * 1000.);
                if scenario == "jump" && prefetch {
                    loop {
                        let (path, completed) =
                            finishes.recv_timeout(Duration::from_secs(3)).unwrap();
                        if path == *b {
                            *cancellation_ms = (*cancellation_ms).max(
                                completed.saturating_duration_since(start).as_secs_f64() * 1000.,
                            );
                            break;
                        }
                    }
                }
            }
        }
        for (
            prefetch,
            Measurements {
                mut samples,
                cancellation_ms,
                hits,
                peak,
            },
        ) in [false, true].into_iter().zip(measurements)
        {
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "{scenario} prefetch={prefetch} median_ms={:.1} max_ms={:.1} ready_hits={hits} peak_sampled_bytes={peak} max_obsolete_decode_ms={cancellation_ms:.1}",
                samples[samples.len() / 2],
                samples.last().unwrap()
            );
        }
    }
}

#[test]
#[ignore = "requires a folder with a page of supported images"]
fn quick_view_cold_page_disk_fill() {
    let directory = PathBuf::from(std::env::var_os("DESHCMD_PREVIEW_BENCH_DIR").unwrap());
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("previews-v1");
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop::default()),
        directory.clone(),
        [directory.clone(), directory],
        true,
    );
    app.previews = Previews::new(ctx.clone(), Some(root.path().to_owned()));
    app.previews.prefetch = true;
    settle(&mut app);
    let expected = app
        .engine
        .tab(Side::Left)
        .visible_entries()
        .filter(|entry| entry.kind == Kind::Image)
        .count();
    assert!((1..MIN_PRELOAD_ROWS).contains(&expected));
    let start = Instant::now();
    let mut frame = eframe::Frame::_new_kittest();
    let mut milestones = [3, 15].into_iter().peekable();
    loop {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(1440., 900.),
                )),
                ..Default::default()
            },
            |ui| {
                app.logic(&ctx, &mut frame);
                app.ui(ui, &mut frame);
            },
        );
        output.textures_delta.clear();
        let count = std::fs::read_dir(&cache)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
                    .count()
            })
            .unwrap_or_default();
        while milestones
            .peek()
            .is_some_and(|&milestone| count >= milestone)
        {
            let milestone = milestones.next().unwrap();
            eprintln!(
                "cached at least {milestone} images in {:.2}s",
                start.elapsed().as_secs_f64()
            );
        }
        if count == expected {
            eprintln!(
                "cached all {count} images in {:.2}s; Quick View closed",
                start.elapsed().as_secs_f64()
            );
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "cached {count} of {expected}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
