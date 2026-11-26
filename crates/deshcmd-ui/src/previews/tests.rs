use super::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

fn image(name: &str) -> Entry {
    Entry {
        path: name.into(),
        archive: None,
        name: name.into(),
        extension: "png".into(),
        kind: Kind::Image,
        size: 1,
        modified: None,
        created: None,
        accessed: None,
        read_only: false,
        link: false,
        hidden: false,
        comment: String::new(),
        attributes: String::new(),
    }
}

struct DecodeJob {
    path: PathBuf,
    cancel: Cancellation,
    finish: mpsc::Sender<Preview>,
}

fn gated_previews() -> (Previews, mpsc::Receiver<DecodeJob>) {
    let (send, jobs) = mpsc::channel();
    let mut previews = Previews::with_decoder(
        egui::Context::default(),
        Arc::new(move |key, _, _, cancel| {
            let (finish, result) = mpsc::channel();
            if send
                .send(DecodeJob {
                    path: key.path.clone(),
                    cancel: cancel.clone(),
                    finish,
                })
                .is_err()
            {
                return Preview::Unavailable;
            }
            result.recv_timeout(TIMEOUT).unwrap_or(Preview::Unavailable)
        }),
    );
    previews.prefetch = true;
    (previews, jobs)
}

fn wait_for(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !done() {
        assert!(Instant::now() < deadline, "preview did not settle");
        std::thread::yield_now();
    }
}

fn pixel() -> Preview {
    Preview::Image {
        size: [1, 1],
        rgba: vec![255; 4],
    }
}

#[test]
fn promotes_running_prediction_and_bypasses_obsolete_decode() {
    let (mut previews, jobs) = gated_previews();
    let (a, b, c, d) = (image("a"), image("b"), image("c"), image("d"));
    previews.update_quick(Some(&a), &[], true, &[]);
    let a_job = jobs.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(a_job.path, a.path);
    a_job.finish.send(pixel()).unwrap();
    wait_for(|| {
        previews.update_quick(Some(&a), &[&b], true, &[]);
        previews.get(&a, QUICK_PIXELS).is_some()
    });
    let b_job = jobs.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(b_job.path, b.path);
    previews.update_quick(Some(&b), &[], true, &[]);
    assert!(!b_job.cancel.cancelled());
    assert!(!previews.focus_pending);
    b_job.finish.send(pixel()).unwrap();
    wait_for(|| {
        previews.update_quick(Some(&b), &[&c], true, &[]);
        previews.get(&b, QUICK_PIXELS).is_some()
    });
    let c_job = jobs.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(c_job.path, c.path, "promotion decoded b twice");

    previews.update_quick(Some(&d), &[&a], true, &[]);
    let d_job = jobs.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(d_job.path, d.path);
    assert!(c_job.cancel.cancelled());
    d_job.finish.send(pixel()).unwrap();
    wait_for(|| {
        previews.update_quick(Some(&d), &[&a], true, &[]);
        previews.get(&d, QUICK_PIXELS).is_some()
    });
    c_job.finish.send(pixel()).unwrap();
    // Request the cancelled key again. The completed old job must not enter the cache.
    previews.update_quick(Some(&c), &[&b], true, &[]);
    let c_retry = jobs.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(c_retry.path, c.path);
    assert!(previews.get(&c, QUICK_PIXELS).is_none());
    c_retry.finish.send(pixel()).unwrap();
}

#[test]
fn focus_replaces_queued_work_while_decoder_is_busy() {
    let (mut previews, jobs) = gated_previews();
    let (a, b, c) = (image("a"), image("b"), image("c"));
    previews.update_quick(Some(&a), &[], true, &[]);
    let first = jobs.recv_timeout(TIMEOUT).unwrap();
    previews.update_quick(Some(&b), &[], true, &[]);
    previews.update_quick(Some(&c), &[], true, &[]);
    assert!(first.cancel.cancelled());
    first.finish.send(pixel()).unwrap();
    let next = jobs.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(next.path, c.path);
    assert!(previews.get(&a, QUICK_PIXELS).is_none());
    assert!(previews.get(&b, QUICK_PIXELS).is_none());
    next.finish.send(pixel()).unwrap();
}

#[test]
fn full_gallery_queue_allows_focus_and_survives_quick_view_close() {
    let (mut previews, jobs) = gated_previews();
    let first = image("gallery");
    assert!(previews.get(&first, GALLERY_PIXELS).is_none());
    let gallery = jobs.recv_timeout(TIMEOUT).unwrap();
    for i in 0..32 {
        previews.get(&image(&format!("thumb-{i}")), GALLERY_PIXELS);
    }
    assert_eq!(previews.gallery_pending.len(), 33);

    let focused = image("focus");
    previews.update_quick(Some(&focused), &[], true, &[]);
    let focus = jobs.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(focus.path, focused.path);
    previews.update_quick(None, &[], false, &[]);
    assert!(focus.cancel.cancelled());
    assert!(!gallery.cancel.cancelled());
    focus.finish.send(pixel()).unwrap();
    gallery.finish.send(pixel()).unwrap();
    wait_for(|| {
        previews.poll_gallery();
        previews.get(&first, GALLERY_PIXELS).is_some()
    });
    assert!(previews.get(&focused, QUICK_PIXELS).is_none());
}

#[test]
fn reverses_prediction_in_visible_order_and_skips_directories() {
    use crate::tests::{TestDesktop, settle};
    use engine::{Command, MatchOptions, Select, Side};

    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().canonicalize().unwrap();
    for name in ["a.png", "b.png", "d.png", "hidden.txt"] {
        std::fs::write(path.join(name), []).unwrap();
    }
    std::fs::create_dir(path.join("c.png")).unwrap();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx);
    let mut app = crate::App::new(
        &creation,
        Box::new(TestDesktop::default()),
        path.clone(),
        [path.clone(), path.clone()],
        true,
    );
    let side = Side::Left;
    app.engine.set_sort_preferences(true, false);
    app.engine
        .dispatch(Command::Filter(side, MatchOptions::filename("*.png")));
    settle(&mut app);

    let (mut previews, jobs) = gated_previews();
    let select = |app: &mut crate::App, name| {
        app.engine
            .dispatch(Command::Select(side, path.join(name), Select::Focus));
    };
    select(&mut app, "b.png");
    previews.update(app.engine.tab(side), false, None);
    assert!(previews.warm_pending.contains_key(&Key::new(
        app.engine.tab(side).current().unwrap(),
        QUICK_PIXELS
    )));
    assert_eq!(previews.warm_pending.len(), 3);
    assert!(
        previews
            .warm_pending
            .keys()
            .all(|key| key.path != path.join("c.png"))
    );
    select(&mut app, "a.png");
    previews.update(app.engine.tab(side), false, None);
    assert!(previews.backwards);
    for _ in 0..2 {
        jobs.recv_timeout(TIMEOUT)
            .unwrap()
            .finish
            .send(pixel())
            .unwrap();
    }
}

#[test]
fn bounds_prediction_bytes_and_cancels_on_revision_change_and_close() {
    let (mut previews, jobs) = gated_previews();
    let mut image = image("b");
    previews.update_quick(Some(&image), &[], false, &[]);
    jobs.recv_timeout(TIMEOUT)
        .unwrap()
        .finish
        .send(Preview::Image {
            size: [1, 1],
            rgba: Vec::with_capacity(SPECULATIVE_BYTES + 1),
        })
        .unwrap();
    wait_for(|| {
        previews.update_quick(Some(&image), &[], false, &[]);
        !previews.warm_rejected.is_empty()
    });
    assert_eq!(previews.warm_bytes, 0);
    assert!(previews.get(&image, QUICK_PIXELS).is_none());

    previews.update_quick(Some(&image), &[], true, &[]);
    let old = jobs.recv_timeout(TIMEOUT).unwrap();
    image.size += 1;
    previews.update_quick(Some(&image), &[], true, &[]);
    assert!(old.cancel.cancelled());
    old.finish.send(pixel()).unwrap();
    let revised = jobs.recv_timeout(TIMEOUT).unwrap();
    assert!(previews.get(&image, QUICK_PIXELS).is_none());
    previews.update_quick(None, &[], false, &[]);
    assert!(revised.cancel.cancelled());
    revised.finish.send(pixel()).unwrap();
}

#[test]
fn opens_warmed_image_and_steps_through_ready_neighbors_without_decoding_again() {
    let (mut previews, jobs) = gated_previews();
    let (a, b, c) = (image("a"), image("b"), image("c"));
    previews.update_quick(Some(&a), &[&b, &c], false, &[]);
    for _ in 0..2 {
        jobs.recv_timeout(TIMEOUT)
            .unwrap()
            .finish
            .send(pixel())
            .unwrap();
        previews.update_quick(Some(&a), &[&b, &c], false, &[]);
    }
    jobs.recv_timeout(TIMEOUT)
        .unwrap()
        .finish
        .send(pixel())
        .unwrap();
    wait_for(|| {
        previews.update_quick(Some(&a), &[&b, &c], false, &[]);
        previews.warm_cache.len() == 3
    });
    assert!(previews.cached.is_empty());
    for entry in [&a, &b, &c] {
        previews.update_quick(Some(entry), &[], true, &[]);
        assert!(matches!(
            previews.get(entry, QUICK_PIXELS),
            Some(CachedPreview::Image(..))
        ));
    }
    assert!(jobs.try_recv().is_err());
}

#[test]
fn opening_queued_preview_moves_it_to_the_focused_worker() {
    let (mut previews, jobs) = gated_previews();
    let (a, b, c) = (image("a"), image("b"), image("c"));
    previews.update_quick(Some(&a), &[&b, &c], false, &[]);
    let first = jobs.recv_timeout(TIMEOUT).unwrap();
    let second = jobs.recv_timeout(TIMEOUT).unwrap();
    assert_ne!(first.path, c.path);
    assert_ne!(second.path, c.path);

    previews.update_quick(Some(&c), &[&a, &b], true, &[]);
    let focused = jobs.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(focused.path, c.path);
    assert!(!first.cancel.cancelled());
    assert!(!second.cancel.cancelled());
    focused.finish.send(pixel()).unwrap();
    wait_for(|| {
        previews.update_quick(Some(&c), &[&a, &b], true, &[]);
        previews.get(&c, QUICK_PIXELS).is_some()
    });
    first.finish.send(pixel()).unwrap();
    second.finish.send(pixel()).unwrap();
}

#[test]
fn completes_the_page_without_filling_the_in_memory_neighbor_cache() {
    let (mut previews, jobs) = gated_previews();
    let entries: Vec<_> = (0..25)
        .map(|index| image(&format!("{index}.png")))
        .collect();
    let page: Vec<_> = entries.iter().collect();
    previews.update_quick(Some(&entries[0]), &[], false, &page);
    let mut decoded = HashSet::new();
    for _ in 0..entries.len() {
        let job = jobs.recv_timeout(TIMEOUT).unwrap();
        assert!(decoded.insert(job.path));
        job.finish.send(pixel()).unwrap();
        previews.update_quick(Some(&entries[0]), &[], false, &page);
    }
    wait_for(|| {
        previews.update_quick(Some(&entries[0]), &[], false, &page);
        previews.page_done.len() == entries.len()
    });
    assert_eq!(previews.warm_cache.len(), 1);
    assert!(previews.warm_pending.is_empty());
    assert!(jobs.try_recv().is_err());
}

#[test]
fn focused_preview_retries_a_speculative_failure() {
    let (mut previews, jobs) = gated_previews();
    let file = image("source.png");
    previews.update_quick(Some(&file), &[], false, &[]);
    let speculative = jobs.recv_timeout(TIMEOUT).unwrap();
    speculative.finish.send(Preview::Unavailable).unwrap();
    wait_for(|| {
        previews.update_quick(Some(&file), &[], false, &[]);
        !previews
            .warm_pending
            .contains_key(&Key::new(&file, QUICK_PIXELS))
    });

    previews.update_quick(Some(&file), &[], true, &[]);
    assert!(previews.get(&file, QUICK_PIXELS).is_none());
    let focused = jobs.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(focused.path, file.path);
    focused.finish.send(pixel()).unwrap();
    wait_for(|| {
        previews.update_quick(Some(&file), &[], true, &[]);
        matches!(
            previews.get(&file, QUICK_PIXELS),
            Some(CachedPreview::Image(..))
        )
    });
}
