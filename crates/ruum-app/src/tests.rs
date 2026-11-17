//! End-to-end tests through the real window harness.
//!
//! These drive the application the way a person does — draw a frame, press
//! keys, type — so they catch what unit tests on the state cannot: a screen
//! that panics while painting, a binding that never fires, a removal that
//! reports success without removing anything.

use std::path::{Path, PathBuf};

use gpui_kit::{
    Bounds, Context, Entity, Pixels, Point, TestAppContext, VisualTestContext,
    px,
};
use gpui_omarchy::Theme;
use ruum_core::removal::RemovalMode;
use ruum_core::scan::{ScanOptions, scan};
use ruum_core::space::{SpaceInfo, Volume};
use ruum_core::treemap::Tile;

use crate::state::{Ruum, Screen};

/// A small tree on disk: two directories, a nested file, and a hidden one.
///
/// The hidden directory holds the largest file, which is both the common case
/// in a home directory and the one the ranking has to get right.
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path();
    for (path, bytes) in [
        ("keep/notes.txt", 1_000_usize),
        ("junk/blob.bin", 200_000),
        ("junk/deeper/more.bin", 100_000),
        (".cache/blob.bin", 300_000),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file, vec![b'x'; bytes]).expect("write");
    }
    temp
}

/// Apparent sizes, so the assertions are about the tree and not about how the
/// filesystem rounds a small file up to a block.
fn options() -> ScanOptions {
    ScanOptions {
        apparent_size: true,
        ..ScanOptions::default()
    }
}

type Window = VisualTestContext;

/// Open a window over a real scan of `root`, without the background walk.
fn view_over<'a>(
    root: &Path,
    cx: &'a mut TestAppContext,
) -> (Entity<Ruum>, &'a mut Window) {
    let tree = scan(root, options()).expect("scan the fixture");
    let root_path = root.to_path_buf();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        Ruum::with_tree(root_path.clone(), tree, options(), 3, cx)
    });
    let focus = view.read_with(cx, |app, _| app.focus.clone());
    cx.update(|window, cx| window.focus(&focus, cx));
    (view, cx)
}

fn draw(cx: &mut Window) {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

fn press(cx: &mut Window, keys: &str) {
    cx.simulate_keystrokes(keys);
    draw(cx);
}

fn update<R>(
    view: &Entity<Ruum>,
    cx: &mut Window,
    f: impl FnOnce(&mut Ruum, &mut Context<'_, Ruum>) -> R,
) -> R {
    view.update(cx, f)
}

fn read<R>(view: &Entity<Ruum>, cx: &Window, f: impl FnOnce(&Ruum) -> R) -> R {
    view.read_with(cx, |app, _| f(app))
}

#[gpui_kit::test]
fn remote_indexing_draws_reading_meter_and_the_sampled_directory(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, _| {
        app.tree = None;
        app.remote = Some("fixture-only".into());
        app.scan_started = Some(std::time::Instant::now());
        app.progress.files = 1234;
        app.progress.bytes = 123_456;
        app.remote_progress.stage = ruum_core::remote_progress::Stage::Indexing;
        app.remote_progress.directory = Some(temp.path().join("junk/deeper"));
    });
    draw(cx);
    assert!(cx.debug_bounds("scan-meter").is_some());
    assert!(cx.debug_bounds("scan-directory").is_some());
    assert!(cx.debug_bounds("scan-reading").is_some());
    update(&view, cx, |app, _| {
        app.scan_error = Some("fixture scan failed".into());
    });
    draw(cx);
    assert!(cx.debug_bounds("scan-error").is_some());
    assert!(cx.debug_bounds("scan-reading").is_none());
}

#[gpui_kit::test]
fn startup_chooser_never_scans_before_explicit_acceptance(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let (view, cx) =
        cx.add_window_view(|_, cx| Ruum::choose(options(), 3, None, cx));
    cx.run_until_parked();
    draw(cx);
    assert!(cx.debug_bounds("choose-root-idle").is_some());
    assert!(cx.debug_bounds("source-host").is_some());
    read(&view, cx, |app| {
        assert!(app.awaiting_root);
        assert_eq!(app.scan_epoch, 0);
        assert!(app.scan.is_none() && app.remote_scan.is_none());
    });
    press(cx, "escape");
    assert!(!read(&view, cx, |app| app.volumes_open));
    press(cx, "r");
    assert!(read(&view, cx, |app| app.volumes_open
        && app.awaiting_root
        && app.scan.is_none()));
}

#[gpui_kit::test]
fn entering_a_directory_scans_only_that_explicit_root(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let selected = fixture();
    let other = fixture();
    let (view, cx) =
        cx.add_window_view(|_, cx| Ruum::choose(options(), 3, None, cx));
    cx.run_until_parked();
    update(&view, cx, |app, _| {
        app.volumes = vec![Volume {
            point: other.path().into(),
            device: None,
            space: None,
        }];
        app.volumes_loading = false;
    });
    draw(cx);
    let input = cx.debug_bounds("source-path").expect("path editor");
    cx.simulate_click(input.center(), gpui_kit::Modifiers::none());
    cx.simulate_input(selected.path().to_str().unwrap());
    assert_eq!(
        update(&view, cx, |app, cx| app
            .picker_path
            .as_ref()
            .unwrap()
            .read(cx)
            .value()
            .to_string()),
        selected.path().to_str().unwrap()
    );
    press(cx, "enter");
    assert_eq!(
        read(&view, cx, |app| app.root_path.clone()),
        dunce::canonicalize(selected.path()).unwrap()
    );
    assert!(!read(&view, cx, |app| app.awaiting_root || app.volumes_open));
    finish_scan(&view, cx);
}

#[gpui_kit::test]
fn invalid_ssh_input_does_not_start_a_scan_or_change_the_source(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let (view, cx) =
        cx.add_window_view(|_, cx| Ruum::choose(options(), 3, None, cx));
    cx.run_until_parked();
    draw(cx);
    let input = cx.debug_bounds("source-host").expect("host editor");
    cx.simulate_click(input.center(), gpui_kit::Modifiers::none());
    cx.simulate_input("invalid host");
    press(cx, "enter");
    read(&view, cx, |app| {
        assert!(app.volumes_error.is_some());
        assert!(app.remote.is_none());
        assert!(app.scan.is_none() && app.remote_scan.is_none());
        assert_eq!(app.scan_epoch, 0);
    });
    press(cx, "escape");
    assert!(!read(&view, cx, |app| app.volumes_open));
}

#[gpui_kit::test]
fn switching_from_remote_to_local_clears_machine_specific_marks(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let target = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.remote = Some("old-source".into());
        app.remote_identity = Some(ruum_core::identity::HostIdentity {
            hostname: "old-source".into(),
            machine_id: None,
        });
        let crumbs = app.crumbs_for_path(&temp.path().join("junk")).unwrap();
        app.select(Some(crumbs), cx);
        app.toggle_mark_selected(cx);
        assert!(!app.marks.is_empty());
        app.volumes_open = true;
        app.volumes_source = None;
        app.choose_source_root(target.path().to_path_buf(), cx);
        assert!(app.remote.is_none() && app.remote_identity.is_none());
        assert!(app.marks.is_empty());
        assert!(app.tree.is_none());
    });
    finish_scan(&view, cx);
}

#[gpui_kit::test]
fn chooser_fits_a_small_window_at_large_interface_zoom(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let (_, cx) =
        cx.add_window_view(|_, cx| Ruum::choose(options(), 3, None, cx));
    cx.run_until_parked();
    cx.simulate_resize(gpui_kit::size(px(900.), px(600.)));
    for rem in [16., 28.] {
        cx.update(|window, _| window.set_rem_size(px(rem)));
        draw(cx);
        let bounds = cx
            .debug_bounds("source-picker")
            .expect("scrollable chooser");
        assert!(bounds.size.width > px(200.), "{bounds:?}");
        assert!(
            bounds.origin.x >= px(0.) && bounds.right() <= px(900.),
            "{bounds:?}"
        );
        assert!(
            bounds.origin.y >= px(0.) && bounds.bottom() <= px(600.),
            "{bounds:?}"
        );
    }
}

#[gpui_kit::test]
fn editing_a_host_never_retargets_an_existing_scan(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, Ruum::open_volumes);
    cx.run_until_parked();
    draw(cx);
    let before = read(&view, cx, |app| app.scan_epoch);
    let input = cx.debug_bounds("source-host").unwrap();
    cx.simulate_click(input.center(), gpui_kit::Modifiers::none());
    cx.simulate_input("new-host-not-connected");
    update(&view, cx, |app, cx| {
        app.choose_source_root(temp.path().into(), cx);
        assert!(app.picker_error.is_some());
        assert!(app.remote.is_none());
        assert_eq!(app.scan_epoch, before);
        assert!(app.tree.is_some());
    });
    press(cx, "escape");
    assert!(!read(&view, cx, |app| app.volumes_open));
}

#[gpui_kit::test]
fn a_directory_error_can_be_corrected_without_reconnecting(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) =
        cx.add_window_view(|_, cx| Ruum::choose(options(), 3, None, cx));
    cx.run_until_parked();
    update(&view, cx, |app, cx| {
        app.choose_source_root(PathBuf::from("relative"), cx);
        assert!(app.picker_error.is_some());
        assert!(app.awaiting_root);
        app.choose_source_root(temp.path().into(), cx);
        assert!(app.picker_error.is_none());
        assert!(!app.awaiting_root);
    });
    finish_scan(&view, cx);
}

#[gpui_kit::test]
fn stale_local_discovery_cannot_overwrite_a_new_source_error(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let (view, cx) =
        cx.add_window_view(|_, cx| Ruum::choose(options(), 3, None, cx));
    // The old discovery is still queued. The new host fails validation
    // without any network connection; only its error may remain.
    update(&view, cx, |app, cx| {
        app.load_volume_source(Some("-invalid".into()), cx);
    });
    cx.run_until_parked();
    draw(cx);
    read(&view, cx, |app| {
        assert_eq!(app.volumes_source.as_deref(), Some("-invalid"));
        assert!(app.volumes_error.is_some());
        assert!(app.volumes.is_empty());
        assert!(app.remote.is_none());
    });
    update(&view, cx, |app, cx| {
        app.close_volumes(cx);
        assert!(!app.poll_roots_once(1, cx));
    });
}

#[gpui_kit::test]
fn remote_root_acceptance_never_stats_the_path_locally(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        let crumbs = app.crumbs_for_path(&temp.path().join("junk")).unwrap();
        app.select(Some(crumbs), cx);
        app.toggle_mark_selected(cx);
        app.volumes_open = true;
        // An intentionally invalid SSH destination ensures acceptance cannot
        // contact any machine. The listing is a decoded fixture, not a walk.
        app.volumes_source = Some("-test-fixture".into());
        app.finish_volume_listing(
            Ok((
                vec![Volume {
                    point: PathBuf::from("/only-on-the-remote-host"),
                    device: Some("/dev/remote-disk".into()),
                    space: None,
                }],
                None,
            )),
            cx,
        );
    });
    draw(cx);
    update(&view, cx, |app, cx| {
        app.choose_volume(cx);
        assert_eq!(app.remote.as_deref(), Some("-test-fixture"));
        assert_eq!(app.root_path, PathBuf::from("/only-on-the-remote-host"));
        assert!(app.marks.is_empty() && app.remote_identity.is_none());
        assert!(
            app.tree.is_none()
                && app.scan.is_none()
                && app.remote_scan.is_none()
        );
        assert!(
            app.scan_error
                .as_ref()
                .is_some_and(|error| error.contains("invalid SSH"))
        );
    });
}

#[gpui_kit::test]
fn repointing_an_ssh_alias_never_reuses_old_machine_marks(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        let old = ruum_core::identity::HostIdentity {
            hostname: "same-name".into(),
            machine_id: Some("machine-a".into()),
        };
        app.remote = Some("stable-alias".into());
        app.update_remote_identity(Some(old.clone()));
        let crumbs = app.crumbs_for_path(&temp.path().join("junk")).unwrap();
        app.select(Some(crumbs), cx);
        app.toggle_mark_selected(cx);
        app.update_remote_identity(Some(old.clone()));
        assert!(!app.marks.is_empty(), "same machine keeps its marks");
        app.update_remote_identity(Some(ruum_core::identity::HostIdentity {
            hostname: old.hostname,
            machine_id: Some("machine-b".into()),
        }));
        assert!(app.marks.is_empty(), "same alias is not proof of identity");
        app.toggle_mark_selected(cx);
        assert!(!app.marks.is_empty());
        app.update_remote_identity(None);
        assert!(app.marks.is_empty(), "lost identity also fails closed");
    });
}

#[gpui_kit::test]
fn remote_review_never_deletes_even_through_direct_actions(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.remote = Some("dev".into());
        let crumbs = app.crumbs_for_path(&temp.path().join("junk")).unwrap();
        app.select(Some(crumbs), cx);
        app.toggle_mark_selected(cx);
        app.screen = Screen::Review;
    });
    draw(cx);
    press(cx, "enter");
    press(cx, "p");
    press(cx, "m");
    update(&view, cx, |app, cx| {
        app.commit(cx);
        app.confirm_delete(cx);
        app.begin_removal(cx);
        assert!(!app.confirm_open);
        assert!(app.run.is_none());
        assert_eq!(app.screen, Screen::Review);
        let selection = app.plan();
        assert_eq!(selection.targets.len(), 1);
        assert!(selection.bytes() > 0);
        assert_eq!(selection.reclaim(), 0);
        assert_eq!(selection.foreign(), 0);
        assert_eq!(selection.unattributed(), selection.bytes());
        app.open_volumes(cx);
        app.restart_as_administrator(cx);
        assert!(!app.volumes_open);
        assert!(!app.restarting);
    });
    draw(cx);
    assert!(temp.path().join("junk/blob.bin").exists());
    assert!(temp.path().join("keep/notes.txt").exists());
}

#[gpui_kit::test]
fn a_failed_remote_scan_stops_showing_the_reading_screen(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, _| {
        app.tree = None;
        app.scan = None;
        app.remote = Some("dev".into());
        app.scan_error =
            Some("SSH dev: Permission denied (publickey,password).".into());
    });
    draw(cx);
    assert!(cx.debug_bounds("scan-error").is_some());
    assert!(cx.debug_bounds("scan-reading").is_none());
}

#[gpui_kit::test]
fn the_window_draws_a_treemap_with_tiles(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    let bounds: Bounds<Pixels> =
        cx.debug_bounds("ruum-root").expect("root element");
    assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));

    let (size, tiles) = update(&view, cx, |app, _| {
        (
            app.treemap_size.get(),
            app.layout().map(<[Tile]>::len).unwrap_or_default(),
        )
    });
    assert!(size.width > px(0.), "treemap width {size:?}");
    assert!(size.height > px(0.), "treemap height {size:?}");
    assert!(tiles >= 3, "tiles laid out: {tiles}");
}

#[gpui_kit::test]
fn the_first_scan_shows_what_it_is_doing_then_the_treemap(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let root = temp.path().to_path_buf();
    // Through the real constructor, so this is the path every first run takes.
    let (view, cx) =
        cx.add_window_view(move |_, cx| Ruum::new(root, options(), 3, cx));
    draw(cx);

    // Nothing is known yet, so the viewport counts the walk instead of showing
    // an empty mosaic.
    assert!(read(&view, cx, |app| app.tree().is_none()));
    assert!(cx.debug_bounds("ruum-root").is_some());

    finish_scan(&view, cx);
    draw(cx);

    let (tiles, selected, hidden_present) = update(&view, cx, |app, _| {
        (
            app.layout().map(<[Tile]>::len).unwrap_or_default(),
            app.selected.is_some(),
            app.tree().is_some_and(|tree| {
                tree.children.iter().any(|c| c.name.starts_with('.'))
            }),
        )
    });
    assert!(tiles >= 3, "tiles after the scan: {tiles}");
    assert!(
        selected,
        "the largest entry is selected for the selection line"
    );
    assert!(hidden_present, "hidden directories are part of the tree");
}

#[gpui_kit::test]
fn keys_walk_the_tree_and_mark_what_is_selected(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    // Largest first: .cache holds the biggest file.
    let (selected, name) = read(&view, cx, |app| {
        (
            app.selected.clone(),
            app.node_at(&[0]).map(|node| node.name.to_string()),
        )
    });
    assert_eq!(selected, Some(vec![0]));
    assert_eq!(name.as_deref(), Some(".cache"));

    press(cx, "space");
    let (marks, bytes) =
        read(&view, cx, |app| (app.marks.len(), app.plan().bytes()));
    assert_eq!(marks, 1);
    assert_eq!(bytes, 300_000, "the whole hidden directory");

    // Enter descends, Escape comes back out.
    press(cx, "enter");
    assert!(!read(&view, cx, |app| app.crumbs.is_empty()));
    press(cx, "escape");
    press(cx, "escape");
    assert!(read(&view, cx, |app| app.crumbs.is_empty()));

    // Space twice more leaves the mark as it was.
    press(cx, "space");
    press(cx, "space");
    let marks = read(&view, cx, |app| app.marks.items().to_vec());
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].bytes, 300_000);
}

#[gpui_kit::test]
fn a_permanent_deletion_asks_in_an_alert_dialog_then_removes(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    // Mark "junk" directly, which is what clicking its tile would do, and
    // choose the permanent path: the trash path would reach the real trash.
    update(&view, cx, |app, cx| {
        app.removal_mode = RemovalMode::Permanent;
        let junk = app
            .tree()
            .and_then(|tree| {
                tree.children
                    .iter()
                    .position(|child| &*child.name == "junk")
                    .map(|index| vec![index])
            })
            .expect("the junk directory");
        app.select(Some(junk.clone()), cx);
        app.toggle_mark(&junk, cx);
        assert_eq!(app.plan().bytes(), 300_000);
    });

    press(cx, "c");
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Review);

    // Enter on the review screen opens the alert dialog; nothing is deleted.
    press(cx, "enter");
    assert!(read(&view, cx, |app| app.confirm_open));
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Review);
    assert!(
        temp.path().join("junk").exists(),
        "nothing has happened yet"
    );

    // Enter in the dialog is its confirm action.
    press(cx, "enter");
    assert!(!read(&view, cx, |app| app.confirm_open));
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Running);

    // The removal runs on a real worker thread, so the test drives the same
    // poll the UI's ticker drives rather than waiting on the test clock.
    let epoch = read(&view, cx, |app| app.run_epoch);
    let mut finished = false;
    for _ in 0..400 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        finished = update(&view, cx, |app, cx| {
            app.poll_removal_once(epoch, cx);
            app.screen == Screen::Done
        });
        if finished {
            break;
        }
    }
    assert!(finished, "the removal finished");
    draw(cx);

    let summary = read(&view, cx, |app| app.run_summary.clone());
    assert_eq!(summary.removed, 1, "{summary:?}");
    assert_eq!(summary.failed, 0, "{summary:?}");
    assert!(
        read(&view, cx, |app| app.marks.is_empty()),
        "the list clears after the run"
    );
    assert!(!temp.path().join("junk").exists(), "junk is gone");
    assert!(
        temp.path().join("keep/notes.txt").exists(),
        "an unmarked directory is untouched"
    );
    assert!(
        temp.path().join(".cache/blob.bin").exists(),
        "an unmarked hidden directory is untouched"
    );
}

/// A subdivided directory keeps a band at the top for its own name, and its
/// children start below it. That is what makes the inner tiles selectable: the
/// parent's name never covers them.
#[gpui_kit::test]
fn a_parents_name_gets_its_own_band_above_its_children(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    let (band, lowest_child_top, bands_in_mosaic) =
        update(&view, cx, |app, _| {
            let tiles = app.layout().map(<[Tile]>::to_vec).unwrap_or_default();
            let parent = tiles
                .iter()
                .find(|tile| tile.crumbs() == [1])
                .expect("the junk directory");
            let band = parent
                .header
                .expect("junk is subdivided, so it keeps a band");
            let children_top = tiles
                .iter()
                .filter(|tile| {
                    tile.crumbs().starts_with(&[1]) && tile.crumbs().len() > 1
                })
                .map(|tile| tile.rect.y)
                .fold(f32::INFINITY, f32::min);
            let mosaic = app.prepare();
            let bands_in_mosaic = mosaic
                .labels
                .iter()
                .filter(|label| label.header.is_some())
                .count();
            (band, children_top, bands_in_mosaic)
        });

    assert!(
        lowest_child_top >= band.bottom() - f32::EPSILON,
        "children start at {} but the band ends at {}",
        lowest_child_top,
        band.bottom()
    );
    assert!(
        band.h >= 8.0,
        "a band has to be tall enough to read: {}",
        band.h
    );
    assert!(
        bands_in_mosaic >= 1,
        "the parent's own label is placed in a band"
    );
}

#[gpui_kit::test]
fn hovering_reports_the_tile_under_the_pointer(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    let (origin, biggest, centre) = update(&view, cx, |app, _| {
        // A leaf tile, so "the deepest tile under the pointer" is unambiguous.
        let biggest = app
            .layout()
            .and_then(|tiles| {
                tiles
                    .iter()
                    .filter(|tile| {
                        let crumbs = tile.crumbs();
                        !tiles.iter().any(|other| {
                            other.crumbs().len() > crumbs.len()
                                && other.crumbs().starts_with(crumbs)
                        })
                    })
                    .max_by(|left, right| {
                        left.rect.area().total_cmp(&right.rect.area())
                    })
                    .map(|tile| tile.crumbs().to_vec())
            })
            .expect("a tile to hover");
        let rect = app.tile_rect(&biggest).expect("a rectangle");
        let screen = app.view.project(rect);
        (
            app.treemap_origin.get(),
            biggest,
            (screen.x + screen.w / 2.0, screen.y + screen.h / 2.0),
        )
    });

    let (x, y) = centre;
    cx.simulate_mouse_move(
        Point::new(origin.x + px(x), origin.y + px(y)),
        None,
        gpui_kit::Modifiers::default(),
    );
    draw(cx);

    let hovered = read(&view, cx, |app| app.hovered.clone());
    assert_eq!(hovered.as_deref(), Some(biggest.as_slice()));
}

#[gpui_kit::test]
fn right_click_selects_the_tile_and_reveals_it(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    let (at, target, path) = update(&view, cx, |app, _| {
        let biggest = app
            .layout()
            .and_then(|tiles| {
                tiles
                    .iter()
                    .max_by(|left, right| {
                        left.rect.area().total_cmp(&right.rect.area())
                    })
                    .map(|tile| tile.crumbs().to_vec())
            })
            .expect("a tile");
        let rect = app.tile_rect(&biggest).expect("a rectangle");
        let screen = app.view.project(rect);
        let (x, y) = (screen.x + screen.w / 2.0, screen.y + screen.h / 2.0);
        // The deepest tile there, which is what a click resolves to.
        let target = app.tile_at(x, y).expect("a tile under the pointer");
        let path = app.path_at(&target).expect("a path");
        let origin = app.treemap_origin.get();
        (Point::new(origin.x + px(x), origin.y + px(y)), target, path)
    });

    // The test platform cannot open a file manager, so take the path away
    // first: the reveal then reports it instead of reaching the platform.
    if path.is_dir() {
        std::fs::remove_dir_all(&path).expect("remove");
    } else {
        std::fs::remove_file(&path).expect("remove");
    }

    cx.simulate_mouse_down(
        at,
        gpui_kit::MouseButton::Right,
        gpui_kit::Modifiers::none(),
    );
    cx.simulate_mouse_up(
        at,
        gpui_kit::MouseButton::Right,
        gpui_kit::Modifiers::none(),
    );
    draw(cx);

    let (selected, notice, expected) = read(&view, cx, |app| {
        let disp = crate::marks::display_path(&path, app.home.as_deref());
        (
            app.selected.clone(),
            app.notice.as_ref().map(|(text, _)| text.clone()),
            crate::i18n::messages::no_longer_on_disk(&disp, app.locale),
        )
    });
    assert_eq!(selected, Some(target), "the clicked tile is selected");
    assert_eq!(notice, Some(expected), "the reveal ran for it");
}

#[gpui_kit::test]
fn typing_filters_live_and_enter_shows_only_the_matches(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);
    let names_drawn = |view: &Entity<Ruum>, cx: &mut Window| {
        update(view, cx, |app, _| {
            let tiles = app.layout().map(<[Tile]>::to_vec).unwrap_or_default();
            tiles
                .iter()
                .filter_map(|tile| app.node_at(tile.crumbs()))
                .map(|node| node.name.to_string())
                .collect::<Vec<_>>()
        })
    };

    press(cx, "/");
    cx.simulate_input("BLOB");
    // The search runs off the UI thread; let it land.
    cx.run_until_parked();
    draw(cx);
    // Live: two matches, nothing hidden yet, the rest only dimmed.
    let (count, applied, keep_filtered) = read(&view, cx, |app| {
        let matches = app.matches.as_deref().expect("matching as it is typed");
        let keep = app.tree().and_then(|tree| {
            tree.children
                .iter()
                .position(|child| &*child.name == "keep")
        });
        (
            matches.count,
            app.filter_applied,
            keep.is_some_and(|index| matches.keep(&[index]).is_none()),
        )
    });
    assert_eq!(count, 2, "junk/blob.bin and .cache/blob.bin");
    assert!(!applied);
    assert!(keep_filtered, "keep holds no blob");
    // Dimmed, not hidden: a non-match is still drawn while typing.
    assert!(names_drawn(&view, cx).contains(&"deeper".to_string()));

    // Enter: only the matches, and the largest selected for marking.
    press(cx, "enter");
    let drawn = names_drawn(&view, cx);
    assert!(read(&view, cx, |app| app.filter_applied));
    assert!(
        !drawn.iter().any(|name| name == "keep" || name == "deeper"),
        "{drawn:?}"
    );
    assert_eq!(
        drawn.iter().filter(|name| *name == "blob.bin").count(),
        2,
        "{drawn:?}"
    );
    let selected = read(&view, cx, |app| {
        app.selected
            .as_deref()
            .and_then(|crumbs| app.path_at(crumbs))
    });
    assert_eq!(selected, Some(temp.path().join(".cache/blob.bin")));

    // Escape gives everything back.
    press(cx, "escape");
    assert!(read(&view, cx, |app| app.matches.is_none()
        && !app.filter_applied));
    assert!(names_drawn(&view, cx).contains(&"deeper".to_string()));
}

#[gpui_kit::test]
fn the_help_overlay_opens_and_closes(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    press(cx, "?");
    assert!(read(&view, cx, |app| app.show_help));
    press(cx, "escape");
    assert!(!read(&view, cx, |app| app.show_help));
}

#[gpui_kit::test]
fn the_volume_picker_opens_moves_and_closes(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    // The picker lists volumes and draws them without panicking; Escape
    // leaves the scan where it was.
    press(cx, "v");
    assert!(read(&view, cx, |app| app.volumes_open));
    draw(cx);
    assert!(cx.debug_bounds("ruum-root").is_some());
    let before = read(&view, cx, |app| app.root_path.clone());
    press(cx, "down");
    press(cx, "up");
    press(cx, "escape");
    assert!(!read(&view, cx, |app| app.volumes_open));
    assert_eq!(read(&view, cx, |app| app.root_path.clone()), before);
}

/// Enter must reach the picker even when its dialog owns keyboard focus.
#[gpui_kit::test]
fn enter_in_the_focused_volume_picker_scans_the_selected_root(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let target = dunce::canonicalize(temp.path().join("junk"))
        .expect("canonical target");
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, _| {
        app.volumes = vec![ruum_core::space::Volume {
            point: target.clone(),
            device: None,
            space: None,
        }];
        app.volume_highlight = 0;
        app.volumes_open = true;
    });
    let focus = read(&view, cx, |app| app.confirm_focus.clone());
    cx.update(|window, cx| window.focus(&focus, cx));
    draw(cx);
    assert!(!read(&view, cx, Ruum::can_start_over));
    press(cx, "enter");
    assert!(!read(&view, cx, |app| app.volumes_open));
    assert_eq!(read(&view, cx, |app| app.root_path.clone()), target);
    finish_scan(&view, cx);
    assert!(read(&view, cx, |app| app.tree().is_some()));
}

#[gpui_kit::test]
fn the_volume_picker_is_centred_and_still_dismisses_from_outside(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    // Fixed rows, so this checks the full-page layout and not which disks
    // the machine running the test happens to have.
    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Sources, cx);
        app.volumes_loading = false;
        app.volumes = vec![
            Volume {
                point: PathBuf::from("/one"),
                device: Some("/dev/one".into()),
                space: Some(SpaceInfo {
                    total: 1_000,
                    free: 500,
                    available: 400,
                }),
            },
            Volume {
                point: PathBuf::from("/two"),
                device: None,
                space: None,
            },
        ];
        cx.notify();
    });
    draw(cx);

    let viewport = cx.update(|window, _| window.viewport_size());
    let page = cx
        .debug_bounds("sources-page")
        .expect("sources full page is drawn");
    let rows = cx.debug_bounds("volume-rows").expect("the rows are drawn");
    let rail = crate::ui::size::ICON_RAIL.to_pixels(px(16.));
    assert!(
        (page.origin.x - rail).abs() < px(2.),
        "sources page starts next to icon rail: {page:?}"
    );
    assert!(
        (page.size.width - (viewport.width - rail)).abs() < px(2.),
        "sources page fills remaining width: {page:?} in {viewport:?}"
    );
    assert!(rows.size.width > px(200.), "{rows:?}");
    assert!(
        cx.debug_bounds("source-cancel").is_none(),
        "full-page Sources view has no modal Cancel button"
    );

    // Clicking empty space on the full-page Sources view does NOT dismiss it.
    cx.simulate_click(
        gpui_kit::point(page.right() - px(8.), page.origin.y + px(8.)),
        gpui_kit::Modifiers::none(),
    );
    draw(cx);
    assert_eq!(
        read(&view, cx, |app| app.screen),
        Screen::Sources,
        "clicking empty space on full-page Sources must not dismiss it"
    );
    assert!(read(&view, cx, |app| app.volumes_open));
}

#[gpui_kit::test]
fn showing_a_tile_that_is_gone_says_so_instead(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    // The selection is .cache, the largest; take it away behind the tree.
    let cache = temp.path().join(".cache");
    std::fs::remove_dir_all(&cache).expect("remove");
    press(cx, "o");
    let (notice, expected) = read(&view, cx, |app| {
        let disp = crate::marks::display_path(&cache, app.home.as_deref());
        (
            app.notice.clone(),
            crate::i18n::messages::no_longer_on_disk(&disp, app.locale),
        )
    });
    let (message, _) = notice.expect("a notice");
    assert_eq!(message, expected);
}

#[gpui_kit::test]
fn command_chords_are_not_read_as_plain_letters(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    // ⌘P and ⌘D are the menu bar's or nobody's; read as `p` and `d` they
    // would hide the selection and re-scan.
    let shown = read(&view, cx, |app| app.show_selection);
    press(cx, "cmd-p cmd-d");
    assert_eq!(read(&view, cx, |app| app.show_selection), shown);
    assert!(read(&view, cx, |app| app.options.apparent_size));
    press(cx, "p");
    assert_eq!(read(&view, cx, |app| app.show_selection), !shown);
}

#[gpui_kit::test]
fn the_treemap_zooms_with_the_wheel_and_resets(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    let middle = read(&view, cx, |app| {
        let size = app.treemap_size.get();
        Point::new(size.width / 2.0, size.height / 2.0)
    });
    update(&view, cx, |app, cx| {
        app.zoom_at(middle.x.as_f32(), middle.y.as_f32(), 1.5, false, cx);
    });
    let zoomed = read(&view, cx, |app| app.view.scale);
    assert!(zoomed > 1.0, "scale after zooming: {zoomed}");

    press(cx, "0");
    assert!((read(&view, cx, |app| app.view.scale) - 1.0).abs() < f32::EPSILON);
}

/// Both appearances Omarchy ships must draw: the palette is derived from the
/// theme, so a light theme is a real second configuration.
#[gpui_kit::test]
fn both_appearances_draw(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    for theme in [Theme::tokyo_night(), Theme::flexoki_light()] {
        view.update(cx, |_, cx| {
            theme.clone().apply(cx);
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("ruum-root").is_some());
    }
}

/// A view built without a scan keeps working: the screens must not assume a
/// walk is in flight.
#[gpui_kit::test]
fn a_view_without_flags_or_marks_still_draws(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.screen = Screen::Review;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("ruum-root").is_some());

    update(&view, cx, |app, cx| {
        app.screen = Screen::Running;
        cx.notify();
    });
    draw(cx);

    update(&view, cx, |app, cx| {
        app.screen = Screen::Done;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("ruum-root").is_some());
}

/// The removal mode is a choice with two independent channels: the key and the
/// button both have to reach the same state.
#[gpui_kit::test]
fn the_review_screen_switches_removal_mode(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.marks.toggle(ruum_core::removal::Target {
            path: temp.path().join("junk"),
            bytes: 300_000,
            is_dir: true,
            hidden: false,
        });
        app.screen = Screen::Review;
        cx.notify();
    });
    draw(cx);

    press(cx, "m");
    assert_eq!(read(&view, cx, |app| app.removal_mode), RemovalMode::Trash);
    press(cx, "p");
    assert_eq!(
        read(&view, cx, |app| app.removal_mode),
        RemovalMode::Permanent
    );
    press(cx, "!");
    assert!(read(&view, cx, |app| app.marks.is_empty()));
}

/// `a` on the review screen copies a prompt for an agent naming the marked
/// path, and removes nothing.
#[gpui_kit::test]
fn the_review_screen_copies_the_list_as_an_agent_prompt(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let junk = temp.path().join("junk");
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.marks.toggle(ruum_core::removal::Target {
            path: junk.clone(),
            bytes: 300_000,
            is_dir: true,
            hidden: false,
        });
        app.screen = Screen::Review;
        cx.notify();
    });
    draw(cx);

    press(cx, "a");
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("a prompt on the clipboard");
    assert!(copied.contains("free up disk space"), "{copied}");
    assert!(
        copied.contains(&format!("- {}", junk.display())),
        "{copied}"
    );
    assert!(junk.exists(), "nothing was removed");
    let (notice, expected) = read(&view, cx, |app| {
        (
            app.notice.clone(),
            crate::i18n::messages::copied_agent_prompt(
                1,
                &ruum_core::size::human_bytes(300_000),
                app.locale,
            ),
        )
    });
    assert_eq!(
        notice.map(|(message, _)| message),
        Some(expected),
        "the copy is confirmed"
    );
}

#[gpui_kit::test]
fn remote_review_copies_host_guarded_script_and_prompt(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.remote = Some("dev-alias".into());
        app.remote_identity = Some(ruum_core::identity::HostIdentity {
            hostname: "actual-server".into(),
            machine_id: Some("abcdefabcdefabcdefabcdefabcdefab".into()),
        });
        app.marks.toggle(ruum_core::removal::Target {
            path: temp.path().join("junk"),
            bytes: 300_000,
            is_dir: true,
            hidden: false,
        });
        app.screen = Screen::Review;
        cx.notify();
    });
    draw(cx);
    press(cx, "y");
    let script = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("script on clipboard");
    assert!(script.contains("actual-server"));
    assert!(script.contains("/etc/machine-id"));
    assert!(script.contains("Wrong hostname"));
    assert!(script.contains("ssh -T"));
    assert!(script.contains("PREVIEW ONLY"));
    press(cx, "a");
    let prompt = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("prompt on clipboard");
    assert!(prompt.contains("actual-server"));
    assert!(prompt.contains("abcdefabcdefabcdefabcdefabcdefab"));
    assert!(prompt.contains("MANDATORY HOST CHECK"));
    assert!(temp.path().join("junk/blob.bin").exists());
}

/// Escape in the alert dialog cancels: the dialog closes, the review screen
/// stays, and nothing on disk changes.
#[gpui_kit::test]
fn escape_in_the_delete_dialog_cancels(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.removal_mode = RemovalMode::Permanent;
        app.marks.toggle(ruum_core::removal::Target {
            path: temp.path().join("junk"),
            bytes: 300_000,
            is_dir: true,
            hidden: false,
        });
        app.screen = Screen::Review;
        cx.notify();
    });
    draw(cx);

    press(cx, "enter");
    assert!(read(&view, cx, |app| app.confirm_open));
    press(cx, "escape");
    assert!(!read(&view, cx, |app| app.confirm_open), "Escape cancels");
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Review);
    assert!(temp.path().join("junk").exists());
    assert_eq!(
        read(&view, cx, |app| app.marks.len()),
        1,
        "the list is kept"
    );
}

/// With a trash on the machine, the reversible path is the default and
/// commits without a dialog; the permanent one is a deliberate choice.
#[gpui_kit::test]
fn the_trash_is_the_default_when_there_is_one(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    let (mode, available) = read(&view, cx, |app| {
        (app.removal_mode, app.trash_backend.is_available())
    });
    if available {
        assert_eq!(mode, RemovalMode::Trash);
    } else {
        assert_eq!(mode, RemovalMode::Permanent);
    }
}

/// `ctrl =` / `ctrl -` / `ctrl 0` change the window's rem, which every size
/// in the app is expressed in, and the mosaic's header band follows it.
#[gpui_kit::test]
fn interface_zoom_scales_the_rem_and_the_header_band(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);
    let rem =
        |cx: &mut Window| cx.update(|window, _| window.rem_size().as_f32());
    let base = rem(cx);
    let band = |view: &Entity<Ruum>, cx: &mut Window| {
        update(view, cx, |app, _| {
            app.layout();
            app.layout_options.header
        })
    };
    let base_band = band(&view, cx);

    press(cx, "ctrl-=");
    let zoomed = rem(cx);
    assert!(zoomed > base, "{zoomed} after zooming in from {base}");
    let zoomed_band = band(&view, cx);
    assert!(
        (zoomed_band / base_band - zoomed / base).abs() < 0.01,
        "the band scales with the rem: {base_band} -> {zoomed_band}"
    );

    press(cx, "ctrl--");
    press(cx, "ctrl--");
    assert!(rem(cx) < base, "zooming out goes below the default");
    press(cx, "ctrl-0");
    assert!((rem(cx) - base).abs() < 0.01, "ctrl 0 resets");
}

/// The crumbs of the directory named `name` directly inside `parent`.
fn child_crumbs(app: &Ruum, parent: &[usize], name: &str) -> Vec<usize> {
    let node = app.node_at(parent).expect("the parent");
    let index = node
        .children
        .iter()
        .position(|child| &*child.name == name)
        .unwrap_or_else(|| panic!("no {name}"));
    let mut crumbs = parent.to_vec();
    crumbs.push(index);
    crumbs
}

/// Regression: the wheel magnified toward the deepest directory under the
/// pointer, then went into the *top-level* one containing it, so the screen
/// after the descent was never the area that was zoomed into.
#[gpui_kit::test]
fn the_wheel_goes_into_the_directory_it_zoomed_into(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    // Point at the middle of junk/deeper's contents: a directory one level
    // below a top-level one.
    let (deeper, point) = update(&view, cx, |app, _| {
        let junk = child_crumbs(app, &[], "junk");
        let deeper = child_crumbs(app, &junk, "deeper");
        let body = app.tile_body(&deeper).expect("deeper is drawn");
        (deeper, (body.x + body.w / 2.0, body.y + body.h / 2.0))
    });

    let mut entered = None;
    for _ in 0..40 {
        let crumbs = update(&view, cx, |app, cx| {
            let (x, y) = app.view.unproject(point.0, point.1);
            let screen = app
                .view
                .project(ruum_core::treemap::Rect::new(x, y, 0.0, 0.0));
            app.zoom_at(screen.x, screen.y, 1.15, true, cx);
            app.crumbs.clone()
        });
        draw(cx);
        if !crumbs.is_empty() {
            entered = Some(crumbs);
            break;
        }
    }
    assert_eq!(
        entered.as_deref(),
        Some(deeper.as_slice()),
        "descended into the pointed-at directory, not its top-level ancestor"
    );
}

/// Regression, keyboard side: Enter on a deep selection enters that
/// directory, and on a file enters the directory holding it.
#[gpui_kit::test]
fn enter_opens_the_selected_directory_at_any_depth(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    let deeper = update(&view, cx, |app, cx| {
        let junk = child_crumbs(app, &[], "junk");
        let deeper = child_crumbs(app, &junk, "deeper");
        app.select(Some(deeper.clone()), cx);
        app.descend(cx);
        deeper
    });
    assert_eq!(read(&view, cx, |app| app.crumbs.clone()), deeper);

    press(cx, "escape");
    press(cx, "escape");
    press(cx, "escape");
    let junk = update(&view, cx, |app, cx| {
        app.go_to(Vec::new(), cx);
        let junk = child_crumbs(app, &[], "junk");
        let blob = child_crumbs(app, &junk, "blob.bin");
        app.select(Some(blob), cx);
        app.descend(cx);
        junk
    });
    assert_eq!(
        read(&view, cx, |app| app.crumbs.clone()),
        junk,
        "a file opens its directory"
    );
}

/// The mark key acts on what the pointer is over when the pointer moved
/// last, and on the keyboard selection after an arrow.
#[gpui_kit::test]
fn the_mark_key_follows_the_pointer_until_the_keyboard_moves(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    // Point at junk's name band, which belongs to junk itself (its body
    // belongs to its children). junk is not the default selection, .cache.
    let (keep, origin, centre) = update(&view, cx, |app, _| {
        let keep = child_crumbs(app, &[], "junk");
        let body = app
            .layout()
            .and_then(|tiles| {
                tiles.iter().find(|tile| tile.crumbs() == keep.as_slice())
            })
            .and_then(|tile| tile.header)
            .expect("junk is drawn with a band");
        (
            keep,
            app.treemap_origin.get(),
            (body.x + body.w / 2.0, body.y + body.h / 2.0),
        )
    });
    assert_ne!(
        read(&view, cx, |app| app.selected.clone()),
        Some(keep.clone())
    );
    cx.simulate_mouse_move(
        Point::new(origin.x + px(centre.0), origin.y + px(centre.1)),
        None,
        gpui_kit::Modifiers::default(),
    );
    draw(cx);

    press(cx, "space");
    let marked = read(&view, cx, |app| app.marks.items().to_vec());
    assert_eq!(marked.len(), 1);
    assert!(
        marked[0].path.ends_with("junk"),
        "marked what the pointer is on: {:?}",
        marked[0].path
    );
    assert_eq!(read(&view, cx, |app| app.selected.clone()), Some(keep));

    // An arrow hands control back to the keyboard selection.
    press(cx, "right");
    assert!(!read(&view, cx, |app| app.pointer_active));
}

/// Regression: after descending, tiles resolved against the scanned root
/// instead of the directory drawn, so labels, hover and marks named strangers.
#[gpui_kit::test]
fn after_descending_every_tile_is_inside_the_directory_drawn(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    let junk = update(&view, cx, |app, cx| {
        let junk = child_crumbs(app, &[], "junk");
        app.select(Some(junk.clone()), cx);
        app.descend(cx);
        junk
    });
    assert_eq!(read(&view, cx, |app| app.crumbs.clone()), junk);
    draw(cx);

    let (paths, labels) = update(&view, cx, |app, _| {
        let tiles = app.layout().map(<[Tile]>::to_vec).unwrap_or_default();
        let paths: Vec<_> = tiles
            .iter()
            .filter_map(|tile| app.path_at(tile.crumbs()))
            .collect();
        let labels: Vec<String> = app
            .prepare()
            .labels
            .into_iter()
            .map(|label| label.text)
            .collect();
        (paths, labels)
    });
    assert!(!paths.is_empty());
    let junk_path = temp.path().join("junk");
    for path in &paths {
        assert!(path.starts_with(&junk_path), "{path:?} is outside junk");
    }
    for label in &labels {
        assert!(
            ["blob.bin", "deeper", "more.bin"].contains(&label.as_str())
                || label.starts_with('+'),
            "label {label} does not belong to junk"
        );
    }

    // Space on the selection marks something inside junk, never elsewhere.
    update(&view, cx, |app, cx| {
        let blob = child_crumbs(app, &junk, "blob.bin");
        app.select(Some(blob), cx);
    });
    press(cx, "space");
    let marked = read(&view, cx, |app| app.marks.items().to_vec());
    assert_eq!(marked.len(), 1);
    assert_eq!(marked[0].path, junk_path.join("blob.bin"));
    assert_eq!(marked[0].bytes, 200_000);

    // And the mark is drawn on the right tile: its crumbs resolve here.
    let hatched = update(&view, cx, |app, _| {
        app.prepare()
            .tiles
            .iter()
            .filter(|tile| tile.marked)
            .count()
    });
    assert_eq!(hatched, 1, "exactly the marked tile is hatched");
}

/// Drive the scan the view started until its outcome has been applied.
///
/// The scan is what is waited on, not the tree: widening keeps the old tree
/// on screen while the wider root is read, so a tree being there does not
/// mean the requested scan has completed.
fn finish_scan(view: &Entity<Ruum>, cx: &mut Window) {
    // Without a scan in flight the poller reports "stopped" at once, and the
    // wait would pass without having waited for anything.
    assert!(
        read(view, cx, |app| app.scan.is_some()),
        "no scan to finish"
    );
    let epoch = read(view, cx, |app| app.scan_epoch);
    for _ in 0..600 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        let running = update(view, cx, |app, cx| app.poll_scan_once(epoch, cx));
        if !running {
            let (error, tree) = read(view, cx, |app| {
                (app.scan_error.clone(), app.tree().is_some())
            });
            assert_eq!(error, None, "the scan failed");
            assert!(tree, "the scan landed without a tree");
            return;
        }
    }
    panic!("the scan never landed");
}

#[gpui_kit::test]
fn dragging_the_panel_edge_resizes_it_within_its_limits(
    cx: &mut TestAppContext,
) {
    use crate::state::{PANEL_MAX_REMS, PANEL_MIN_REMS, panel_width};
    use gpui_kit::{Modifiers, MouseButton};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1400.), px(900.)));
    draw(cx);
    let handle = cx.debug_bounds("panel-handle").expect("the panel is shown");
    let start = handle.center();
    let before = read(&view, cx, |app| app.panel_rems);

    // A drag starts on the first move and reports moves after it, as a real
    // pointer does in many small steps.
    let drag = |cx: &mut Window, from: Point<Pixels>, to: Point<Pixels>| {
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        for step in 1..=4 {
            let t = step as f32 / 4.0;
            let at = Point::new(from.x + (to.x - from.x) * t, to.y);
            cx.simulate_mouse_move(
                at,
                Some(MouseButton::Left),
                Modifiers::none(),
            );
        }
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        draw(cx);
    };

    // Wider: drag the edge 160 px to the left.
    let wider = Point::new(start.x - px(160.), start.y);
    drag(cx, start, wider);
    let after = read(&view, cx, |app| app.panel_rems);
    assert!(
        (after - before - 10.0).abs() < 0.5,
        "160 px is 10 rem: {before} -> {after}"
    );

    // The limits, for any edge position: a minimum, a maximum, and never
    // so wide that the mosaic gets less room than the panel's own minimum.
    assert!((panel_width(1390., 1400., 16.) - PANEL_MIN_REMS).abs() < 1e-3);
    assert!((panel_width(0., 3000., 16.) - PANEL_MAX_REMS).abs() < 1e-3);
    let squeezed = panel_width(0., 800., 16.);
    assert!((squeezed - (800. / 16. - PANEL_MIN_REMS)).abs() < 1e-3);
    assert!((panel_width(0., 300., 16.) - PANEL_MIN_REMS).abs() < 1e-3);
}

#[gpui_kit::test]
fn widening_reuses_the_tree_it_has_and_reads_only_the_rest(
    cx: &mut TestAppContext,
) {
    use crate::state::Crumb;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let inner = temp.path().join("junk");
    let (view, cx) = view_over(&inner, cx);
    update(&view, cx, |app, _| {
        app.disk_root = Some(temp.path().to_path_buf());
        // Stale on purpose: the wider root, a folder, must reset it.
        app.file_table = true;
    });
    let before = read(&view, cx, |app| app.tree().map(|tree| tree.files));

    // The trail runs from the top of the filesystem — "/", or a drive such
    // as "C:\" — and the scanned root sits under its parents.
    let trail = read(&view, cx, Ruum::breadcrumbs);
    let top = temp.path().ancestors().last().expect("a top");
    assert_eq!(trail[0].0, top.display().to_string());
    assert!(
        trail.contains(&(
            temp.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            Crumb::Above(temp.path().to_path_buf())
        ))
    );

    // Written after the first scan: a memoized subtree cannot see it.
    std::fs::write(inner.join("late.bin"), vec![0_u8; 4096]).expect("write");

    press(cx, "g");
    // The old tree stays on screen while the wider one is read.
    assert!(read(&view, cx, |app| app.tree().is_some()));
    finish_scan(&view, cx);
    assert_eq!(read(&view, cx, |app| app.root_path.clone()), temp.path());
    assert!(
        !read(&view, cx, |app| app.file_table),
        "the offer follows the root it widened to"
    );
    let (reused, rest, selected) = read(&view, cx, |app| {
        let tree = app.tree().expect("the wider tree");
        let junk = tree.child_named("junk").map(|node| node.files);
        let selected = app
            .selected
            .as_deref()
            .and_then(|crumbs| app.node_at(crumbs))
            .map(|node| node.name.to_string());
        (junk, tree.child_named("keep").is_some(), selected)
    });
    assert_eq!(reused, before, "junk was reused, not walked again");
    assert!(rest, "what is outside it was walked");
    assert_eq!(selected.as_deref(), Some("junk"), "where you came from");
}

#[gpui_kit::test]
fn enter_before_the_search_lands_applies_it_when_it_does(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    press(cx, "/");
    cx.simulate_input("blob");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    draw(cx);
    let (applied, count, open) = read(&view, cx, |app| {
        (
            app.filter_applied,
            app.matches.as_deref().map(|matches| matches.count),
            app.find_open,
        )
    });
    assert!(applied, "Enter was not lost to a search in flight");
    assert_eq!(count, Some(2));
    assert!(!open);
}

#[gpui_kit::test]
fn a_crumb_lists_its_siblings_and_jumps_sideways(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1400.), px(900.)));
    draw(cx);
    // Into junk, so the trail ends in a crumb that has siblings.
    let junk = read(&view, cx, |app| {
        app.tree()
            .and_then(|tree| {
                tree.children.iter().position(|c| &*c.name == "junk")
            })
            .expect("junk")
    });
    update(&view, cx, |app, cx| app.go_to(vec![junk], cx));
    draw(cx);

    let last = read(&view, cx, |app| app.breadcrumbs().len() - 1);
    let chevron = cx
        .debug_bounds(Box::leak(format!("crumb-{last}-menu").into_boxed_str()))
        .expect("the current crumb has a menu");
    cx.simulate_click(chevron.center(), Modifiers::none());
    draw(cx);
    assert!(
        cx.debug_bounds("sibling-menu").is_some(),
        "the menu is drawn"
    );
    let (names, highlighted) = read(&view, cx, |app| {
        let menu = app.crumb_menu.clone().expect("open");
        let (rows, _) = app.siblings(&menu.parent);
        let names: Vec<String> =
            rows.iter().map(|row| row.name.clone()).collect();
        let highlighted = names_at(&names, menu.highlighted);
        (names, highlighted)
    });
    assert!(
        names.contains(&".cache".to_string())
            && names.contains(&"keep".to_string())
    );
    assert_eq!(highlighted, "junk", "it opens on where you are");

    // Walk to .cache by name, not by rank: it and junk are close in size.
    let target = names
        .iter()
        .position(|name| name == ".cache")
        .expect("listed");
    let here = names
        .iter()
        .position(|name| name == "junk")
        .expect("listed");
    let key = if target < here { "up" } else { "down" };
    for _ in 0..target.abs_diff(here) {
        press(cx, key);
    }
    press(cx, "enter");
    let (crumbs, open) = read(&view, cx, |app| {
        (app.crumbs.clone(), app.crumb_menu.is_some())
    });
    let cache = read(&view, cx, |app| {
        app.tree()
            .and_then(|tree| {
                tree.children.iter().position(|c| &*c.name == ".cache")
            })
            .expect(".cache")
    });
    assert_eq!(crumbs, vec![cache], "went sideways into .cache");
    assert!(!open);

    // A click outside closes it.
    let chevron = cx
        .debug_bounds(Box::leak(format!("crumb-{last}-menu").into_boxed_str()))
        .expect("menu chevron");
    cx.simulate_click(chevron.center(), Modifiers::none());
    draw(cx);
    assert!(read(&view, cx, |app| app.crumb_menu.is_some()));
    cx.simulate_mouse_down(
        gpui_kit::point(px(700.), px(600.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        gpui_kit::point(px(700.), px(600.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    draw(cx);
    assert!(read(&view, cx, |app| app.crumb_menu.is_none()));
}

fn names_at(names: &[String], index: usize) -> String {
    names.get(index).cloned().unwrap_or_default()
}

#[gpui_kit::test]
fn marking_a_directory_marks_everything_inside_it(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);
    let crumbs = |view: &Entity<Ruum>, cx: &mut Window, rel: &str| {
        let path = temp.path().join(rel);
        read(view, cx, |app| app.crumbs_for_path(&path)).expect(rel)
    };
    let junk = crumbs(&view, cx, "junk");
    let deeper = crumbs(&view, cx, "junk/deeper");

    // Marked inside first, then the directory around it: one mark remains,
    // and it covers the inner one.
    update(&view, cx, |app, cx| app.toggle_mark(&deeper, cx));
    update(&view, cx, |app, cx| app.toggle_mark(&junk, cx));
    let marks = read(&view, cx, |app| {
        app.marks
            .items()
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        marks,
        vec![temp.path().join("junk")],
        "the inner mark is absorbed"
    );

    // Every tile inside it is drawn marked: the ones below the mark are
    // "covered", which paints the same fill and label.
    let mosaic = update(&view, cx, |app, _| app.prepare());
    let (inside, covered) = update(&view, cx, |app, _| {
        let tiles = app.layout().map(<[Tile]>::to_vec).unwrap_or_default();
        let inside: Vec<bool> = tiles
            .iter()
            .zip(&mosaic.tiles)
            .filter(|(tile, _)| {
                tile.crumbs().starts_with(&junk) && tile.crumbs() != junk
            })
            .map(|(_, deco)| deco.covered)
            .collect();
        (inside.len(), inside.iter().all(|covered| *covered))
    });
    assert!(inside >= 2, "blob.bin and deeper are drawn inside junk");
    assert!(covered, "every tile inside junk goes with it");

    // Marking something inside it is refused, and says why.
    update(&view, cx, |app, cx| app.toggle_mark(&deeper, cx));
    let (count, notice, expected) = read(&view, cx, |app| {
        let deeper_path = temp.path().join("junk").join("deeper");
        let junk_path = temp.path().join("junk");
        let target_disp =
            crate::marks::display_path(&deeper_path, app.home.as_deref());
        let ancestor_disp =
            crate::marks::display_path(&junk_path, app.home.as_deref());
        (
            app.marks.len(),
            app.notice.as_ref().map(|(text, _)| text.clone()),
            crate::i18n::messages::goes_with_marked(
                &target_disp,
                &ancestor_disp,
                app.locale,
            ),
        )
    });
    assert_eq!(count, 1);
    assert_eq!(notice, Some(expected));

    // Unmarking the directory unmarks everything.
    update(&view, cx, |app, cx| app.toggle_mark(&junk, cx));
    assert!(read(&view, cx, |app| app.marks.is_empty()));
}

/// `<` and `>` retrace the directories visited, are disabled when there is
/// nowhere to go, and a fresh move ends what was ahead.
#[gpui_kit::test]
fn back_and_forward_retrace_where_you_have_been(cx: &mut TestAppContext) {
    use gpui_kit::Modifiers;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1400.), px(900.)));
    draw(cx);
    let can = |view: &Entity<Ruum>, cx: &Window| {
        read(view, cx, |app| (app.can_go_back(), app.can_go_forward()))
    };
    assert_eq!(can(&view, cx), (false, false), "nowhere to go yet");

    let (junk, deeper, keep) = update(&view, cx, |app, cx| {
        let junk = child_crumbs(app, &[], "junk");
        let deeper = child_crumbs(app, &junk, "deeper");
        let keep = child_crumbs(app, &[], "keep");
        app.select(Some(deeper.clone()), cx);
        app.descend(cx);
        (junk, deeper, keep)
    });
    draw(cx);
    assert_eq!(can(&view, cx), (true, false));

    // Hovering `<` shows the card for where it goes, as hovering a tile
    // does: here, the scanned root. It hangs below the button, never on it.
    let back = cx
        .debug_bounds("history-back")
        .expect("the button is drawn");
    cx.simulate_mouse_move(back.center(), None, Modifiers::none());
    draw(cx);
    let card = cx.debug_bounds("history-tip").expect("the card is shown");
    assert!(card.top() >= back.bottom(), "{card:?} covers {back:?}");
    assert_eq!(
        read(&view, cx, |app| app.history_target(true)),
        Some((0, Vec::new()))
    );

    let click = |cx: &mut Window, selector: &'static str| {
        let bounds = cx.debug_bounds(selector).expect("the button is drawn");
        cx.simulate_click(bounds.center(), Modifiers::none());
        draw(cx);
    };
    click(cx, "history-back");
    assert_eq!(
        read(&view, cx, |app| app.crumbs.clone()),
        Vec::<usize>::new()
    );
    assert_eq!(can(&view, cx), (false, true), "back at the start");

    click(cx, "history-forward");
    assert_eq!(read(&view, cx, |app| app.crumbs.clone()), deeper);
    assert_eq!(can(&view, cx), (true, false));

    // Up a level is a move of its own, and the keys retrace it too.
    press(cx, "backspace");
    assert_eq!(read(&view, cx, |app| app.crumbs.clone()), junk);
    press(cx, "alt-left");
    assert_eq!(read(&view, cx, |app| app.crumbs.clone()), deeper);
    press(cx, "alt-left");
    assert_eq!(
        read(&view, cx, |app| app.crumbs.clone()),
        Vec::<usize>::new()
    );
    press(cx, "alt-right");
    assert_eq!(read(&view, cx, |app| app.crumbs.clone()), deeper);

    // Going somewhere new from the middle of the history drops what was
    // ahead of it, as in a browser.
    press(cx, "alt-left");
    update(&view, cx, |app, cx| app.go_to(keep, cx));
    draw(cx);
    assert_eq!(can(&view, cx), (true, false), "the forward trail is gone");
    press(cx, "alt-left");
    assert_eq!(
        read(&view, cx, |app| app.crumbs.clone()),
        Vec::<usize>::new()
    );
}

/// Regression: children are ordered by the metric, so switching between
/// Size and Files reorders them. The directory on screen and the selection
/// are found again by path, rather than following their old positions into
/// a sibling.
#[gpui_kit::test]
fn switching_the_metric_keeps_the_directory_and_the_selection(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path();
    // Largest by size, but fewest files: the two swap places.
    std::fs::create_dir_all(root.join("big")).expect("mkdir");
    std::fs::write(root.join("big/one.bin"), vec![b'x'; 500_000])
        .expect("write");
    std::fs::write(root.join("big/two.bin"), vec![b'x'; 1_000]).expect("write");
    for index in 0..5 {
        let file = root.join(format!("many/{index}.txt"));
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file, b"x").expect("write");
    }
    let (view, cx) = view_over(root, cx);
    draw(cx);

    update(&view, cx, |app, cx| {
        let big = child_crumbs(app, &[], "big");
        assert_eq!(big, vec![0], "largest first by size");
        app.go_to(big.clone(), cx);
        let one = child_crumbs(app, &big, "one.bin");
        app.select(Some(one), cx);
    });
    draw(cx);

    update(&view, cx, |app, cx| app.set_mode(1, cx));
    draw(cx);
    let (here, selected) = read(&view, cx, |app| {
        (
            app.current_path(),
            app.selected
                .as_deref()
                .and_then(|crumbs| app.path_at(crumbs)),
        )
    });
    assert_eq!(here, root.join("big"));
    assert_eq!(selected, Some(root.join("big/one.bin")));

    update(&view, cx, |app, cx| app.set_mode(0, cx));
    draw(cx);
    assert_eq!(read(&view, cx, Ruum::current_path), root.join("big"));
}

/// Escape stops a first scan. What the walk found so far is not shown as a
/// tree, a late result is ignored, and `r` starts over.
#[gpui_kit::test]
fn escape_cancels_the_first_scan_and_r_starts_it_again(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let root = temp.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(move |_, cx| Ruum::new(root, options(), 3, cx));
    let focus = view.read_with(cx, |app, _| app.focus.clone());
    cx.update(|window, cx| window.focus(&focus, cx));
    draw(cx);
    let epoch = read(&view, cx, |app| app.scan_epoch);

    press(cx, "escape");
    let (scanning, cancelled) = read(&view, cx, |app| {
        (app.scan.is_some(), app.progress.cancelled)
    });
    assert!(!scanning, "the walk was dropped");
    assert!(cancelled, "the panel can say it stopped");
    std::thread::sleep(std::time::Duration::from_millis(50));
    let polling = update(&view, cx, |app, cx| app.poll_scan_once(epoch, cx));
    assert!(!polling, "the old poller stops");
    assert!(read(&view, cx, |app| app.tree().is_none()));
    draw(cx);

    press(cx, "r");
    finish_scan(&view, cx);
    assert!(read(&view, cx, |app| app.tree().is_some()));
}

/// Escape stops a widening scan and keeps the tree it started from.
#[gpui_kit::test]
fn escape_cancels_widening_and_keeps_the_tree_on_screen(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let inner = temp.path().join("junk");
    let (view, cx) = view_over(&inner, cx);
    update(&view, cx, |app, _| {
        app.disk_root = Some(temp.path().to_path_buf());
    });
    let before = read(&view, cx, |app| app.tree().map(|tree| tree.files));

    press(cx, "g");
    assert!(read(&view, cx, |app| app.scan.is_some()));
    press(cx, "escape");
    let (scanning, root, scan_root, files) = read(&view, cx, |app| {
        (
            app.scan.is_some(),
            app.root_path.clone(),
            app.scan_root.clone(),
            app.tree().map(|tree| tree.files),
        )
    });
    assert!(!scanning);
    assert_eq!(root, inner);
    assert_eq!(scan_root, inner, "the trail stops showing a widening");
    assert_eq!(files, before, "the tree on screen is unchanged");
}

/// The mouse's back and forward buttons retrace the same history as
/// alt-arrows and the header buttons, and only on the explore screen.
#[gpui_kit::test]
fn mouse_side_buttons_go_back_and_forward(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, NavigationDirection};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1400.), px(900.)));
    draw(cx);

    let (junk, deeper) = update(&view, cx, |app, cx| {
        let junk = child_crumbs(app, &[], "junk");
        let deeper = child_crumbs(app, &junk, "deeper");
        app.select(Some(junk.clone()), cx);
        app.descend(cx);
        app.select(Some(deeper.clone()), cx);
        app.descend(cx);
        (junk, deeper)
    });
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.crumbs.clone()), deeper);

    // Over the mosaic, buttons 8 and 9 step the history, as alt-arrows do.
    let mosaic = cx.debug_bounds("treemap").expect("the mosaic is drawn");
    let at = mosaic.center();
    let press_button = |cx: &mut Window, button: MouseButton| {
        cx.simulate_mouse_move(at, None, Modifiers::none());
        cx.simulate_mouse_down(at, button, Modifiers::none());
        cx.simulate_mouse_up(at, button, Modifiers::none());
        draw(cx);
    };
    press_button(cx, MouseButton::Navigate(NavigationDirection::Back));
    assert_eq!(read(&view, cx, |app| app.crumbs.clone()), junk);
    press_button(cx, MouseButton::Navigate(NavigationDirection::Forward));
    assert_eq!(read(&view, cx, |app| app.crumbs.clone()), deeper);

    // On the review screen the same press changes nothing: the marked list
    // is not somewhere the history can take you back to.
    update(&view, cx, |app, cx| app.toggle_mark(&junk, cx));
    press(cx, "c");
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Review);
    press_button(cx, MouseButton::Navigate(NavigationDirection::Back));
    assert_eq!(read(&view, cx, |app| app.crumbs.clone()), deeper);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Review);
}

/// The restart as administrator reopens the same root and options: every
/// flag it writes is one the command line reads back.
#[test]
fn restart_arguments_parse_back_to_the_same_scan() {
    use ruum_core::tree::Metric;
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().join("a folder");
    std::fs::create_dir(&root).expect("mkdir");
    let root = dunce::canonicalize(&root).expect("canonical root");
    let changed = ScanOptions {
        apparent_size: true,
        follow_links: true,
        include_hidden: false,
        one_filesystem: false,
        metric: Metric::Files,
        threads: ruum_core::scan_threads::ScanThreads {
            max_threads: 8,
            adaptive: true,
            retained_throughput: 0.85,
            system_cpu_limit: Some(0.70),
        },
        ..ScanOptions::default()
    };
    for (options, depth) in [(changed, 5), (ScanOptions::default(), 1)] {
        let args = crate::state::restart_args(&options, depth, &root);
        let parsed = crate::parse_args(args.into_iter()).expect("parses");
        assert_eq!(parsed.root, root);
        assert_eq!(parsed.depth, depth);
        let got = &parsed.options;
        assert_eq!(got.threads.max_threads, options.threads.max_threads);
        assert_eq!(got.threads.adaptive, options.threads.adaptive);
        assert!(
            (got.threads.retained_throughput
                - options.threads.retained_throughput)
                .abs()
                < f64::EPSILON
        );
        assert_eq!(
            got.threads.system_cpu_limit,
            options.threads.system_cpu_limit
        );
        assert_eq!(
            (
                got.apparent_size,
                got.follow_links,
                got.include_hidden,
                got.one_filesystem,
                got.metric,
            ),
            (
                options.apparent_size,
                options.follow_links,
                options.include_hidden,
                options.one_filesystem,
                options.metric,
            )
        );
    }
}

#[gpui_kit::test]
fn power_efficiency_menu_saves_without_discarding_the_tree(
    cx: &mut TestAppContext,
) {
    use crate::power::{self, PowerEfficiency as Power};
    use gpui_kit::Modifiers;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let config = tempfile::tempdir().expect("config");
    let path = config.path().join("power-efficiency");
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(900.), px(600.)));
    update(&view, cx, |app, _| {
        app.power_settings_path = Some(path.clone());
        // Enough CPUs that every preset is offered, whatever runs the test.
        app.cpu_threads = 18;
    });
    let epoch = read(&view, cx, |app| app.scan_epoch);
    for preset in Power::ALL {
        draw(cx);
        let button = cx.debug_bounds("power-efficiency").expect("control");
        cx.simulate_click(button.center(), Modifiers::none());
        draw(cx);
        let choice = cx.debug_bounds(preset.key()).expect("preset");
        cx.simulate_click(choice.center(), Modifiers::none());
        draw(cx);
        assert_eq!(power::load(&path).expect("saved"), preset);
        read(&view, cx, |app| {
            assert_eq!(app.power_choice, Some(preset));
            assert_eq!(
                app.options.threads.max_threads,
                preset.threads(app.cpu_threads)
            );
            assert!(!app.options.threads.adaptive);
            assert!(app.power_menu.is_none());
            assert_eq!(app.scan_epoch, epoch);
            assert!(app.tree.is_some());
        });
    }
    // Persistence failure must not masquerade as a saved preference.
    update(&view, cx, |app, cx| {
        app.power_settings_path = Some(config.path().to_owned());
        app.set_power_efficiency(Power::Miser, cx);
    });
    assert!(read(&view, cx, |app| {
        let label = Power::Miser.label_in(app.cpu_threads, app.locale);
        let prefix =
            crate::i18n::messages::power_save_failed(&label, "", app.locale);
        // Compare without the OS-specific trailing error text.
        let head = prefix.trim_end();
        app.notice
            .as_ref()
            .is_some_and(|(message, _)| message.starts_with(head))
    }));
    update(&view, cx, Ruum::start_scan);
    assert_eq!(read(&view, cx, |app| app.scan_epoch), epoch + 1);
}

#[gpui_kit::test]
fn the_power_menu_offers_only_what_the_cpus_can_tell_apart(
    cx: &mut TestAppContext,
) {
    use crate::power::PowerEfficiency as Power;
    use gpui_kit::Modifiers;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let config = tempfile::tempdir().expect("config");
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(900.), px(600.)));
    update(&view, cx, |app, _| {
        app.power_settings_path = Some(config.path().join("power"));
        // Aggressive and Drain My Battery would both run four workers.
        app.cpu_threads = 4;
        app.power_choice = Some(Power::Balanced);
    });
    draw(cx);
    let toggle = cx.debug_bounds("power-efficiency").expect("control");
    cx.simulate_mouse_move(toggle.center(), None, Modifiers::none());
    cx.simulate_click(toggle.center(), Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.power_menu), Some(1));

    // The pointer takes the highlight, and the check stays on what is saved.
    let miser = cx.debug_bounds(Power::Miser.key()).expect("row");
    cx.simulate_mouse_move(miser.center(), None, Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.power_menu), Some(0));
    let balanced = cx.debug_bounds(Power::Balanced.key()).expect("row");
    let check = cx.debug_bounds("power-check").expect("check");
    assert!(balanced.contains(&check.center()), "check on {check:?}");

    // A disabled row ignores the click and leaves the menu to be dismissed.
    let drain = cx.debug_bounds(Power::DrainMyBattery.key()).expect("row");
    cx.simulate_click(drain.center(), Modifiers::none());
    draw(cx);
    read(&view, cx, |app| {
        assert_eq!(app.power_choice, Some(Power::Balanced));
        assert!(app.notice.is_none());
    });

    // The arrows step over what is not offered, and Enter chooses.
    update(&view, cx, |app, cx| {
        app.power_menu = None;
        app.toggle_power_menu(cx);
    });
    press(cx, "down");
    assert_eq!(read(&view, cx, |app| app.power_menu), Some(1));
    press(cx, "up");
    assert_eq!(read(&view, cx, |app| app.power_menu), Some(0));
    press(cx, "enter");
    read(&view, cx, |app| {
        assert_eq!(app.power_choice, Some(Power::Miser));
        assert_eq!(app.options.threads.max_threads, 2);
        assert!(app.power_menu.is_none());
    });

    // The button closes what it opened, rather than the press outside the
    // menu closing it only for the click to open it again.
    draw(cx);
    let toggle = cx.debug_bounds("power-efficiency").expect("control");
    cx.simulate_mouse_move(toggle.center(), None, Modifiers::none());
    cx.simulate_click(toggle.center(), Modifiers::none());
    draw(cx);
    assert!(read(&view, cx, |app| app.power_menu.is_some()));
    cx.simulate_click(toggle.center(), Modifiers::none());
    draw(cx);
    assert!(read(&view, cx, |app| app.power_menu.is_none()));
}

#[gpui_kit::test]
fn icon_rail_switches_pages_and_settings_shortcut(cx: &mut TestAppContext) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    // Initial page is Explore
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);
    assert!(cx.debug_bounds("icon-rail").is_some());
    assert!(cx.debug_bounds("rail-explore").is_some());
    assert!(cx.debug_bounds("rail-sources").is_some());
    assert!(cx.debug_bounds("rail-watchdog").is_some());
    assert!(cx.debug_bounds("rail-settings").is_some());

    // Explore -> Sources
    let sources_btn = cx.debug_bounds("rail-sources").expect("rail-sources");
    cx.simulate_click(sources_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Sources);
    assert!(cx.debug_bounds("sources-page").is_some());

    // Sources -> Watchdog
    let watchdog_btn = cx.debug_bounds("rail-watchdog").expect("rail-watchdog");
    cx.simulate_click(watchdog_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Watchdog);
    assert!(cx.debug_bounds("watchdog-page").is_some());

    // Watchdog -> Sources (bidirectional click)
    let sources_btn = cx.debug_bounds("rail-sources").expect("rail-sources");
    cx.simulate_click(sources_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Sources);
    assert!(cx.debug_bounds("sources-page").is_some());

    // Sources -> Settings
    let settings_btn = cx.debug_bounds("rail-settings").expect("rail-settings");
    cx.simulate_click(settings_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Settings);
    assert!(cx.debug_bounds("settings-page").is_some());

    // Settings -> Sources (bidirectional click)
    let sources_btn = cx.debug_bounds("rail-sources").expect("rail-sources");
    cx.simulate_click(sources_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Sources);
    assert!(cx.debug_bounds("sources-page").is_some());

    // Sources -> Explore
    let explore_btn = cx.debug_bounds("rail-explore").expect("rail-explore");
    cx.simulate_click(explore_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);

    // Settings -> Escape returns to Explore
    let settings_btn = cx.debug_bounds("rail-settings").expect("rail-settings");
    cx.simulate_click(settings_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Settings);
    press(cx, "escape");
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);

    // Menu actions switch pages directly
    cx.dispatch_action(crate::app_menu::OpenWatchdog);
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Watchdog);

    cx.dispatch_action(crate::app_menu::OpenSources);
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Sources);

    cx.dispatch_action(crate::app_menu::OpenExplore);
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);

    cx.dispatch_action(crate::app_menu::OpenSettings);
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Settings);
}

#[test]
fn cleanup_confirm_modal_selection_and_decision() {
    use ruum_core::confirm::{
        CleanupConfirmDecision, CleanupConfirmItem, CleanupConfirmRequest,
    };
    use std::sync::mpsc::channel;

    let path_a = PathBuf::from("/tmp/a.cache");
    let path_b = PathBuf::from("/tmp/b.log");
    let path_c = PathBuf::from("/tmp/c.tmp");

    let request = CleanupConfirmRequest {
        root: PathBuf::from("/tmp"),
        title: "Test Cleanup".into(),
        summary: Some("Summary".into()),
        items: vec![
            CleanupConfirmItem {
                path: path_a.clone(),
                bytes: 100,
                mode: RemovalMode::Trash,
                reason: Some("cache".into()),
                git_warning: None,
            },
            CleanupConfirmItem {
                path: path_b.clone(),
                bytes: 200,
                mode: RemovalMode::Permanent,
                reason: Some("old log".into()),
                git_warning: Some("untracked file".into()),
            },
            CleanupConfirmItem {
                path: path_c.clone(),
                bytes: 300,
                mode: RemovalMode::Trash,
                reason: Some("temporary".into()),
                git_warning: None,
            },
        ],
    };

    let (resp_tx, resp_rx) = channel();
    let mut modal =
        crate::state::CleanupConfirmModal::new(request, Some(resp_tx), None);

    // Invariant: all items checked by default
    assert!(modal.is_checked(&path_a));
    assert!(modal.is_checked(&path_b));
    assert!(modal.is_checked(&path_c));
    assert_eq!(modal.selected_count(), 3);
    assert_eq!(modal.selected_bytes(), 600);

    // Uncheck item B (e.g. user unchecks untracked file)
    modal.toggle_path(&path_b);
    assert!(!modal.is_checked(&path_b));
    assert_eq!(modal.selected_count(), 2);
    assert_eq!(modal.selected_bytes(), 400);

    // Confirm: only checked paths are returned
    let decision = modal.confirm();
    assert_eq!(
        decision,
        CleanupConfirmDecision::Confirmed {
            selected_paths: vec![path_a, path_c],
        }
    );

    // IPC responder received the matching decision
    let received = resp_rx.recv().expect("recv decision");
    assert_eq!(received, decision);
}

#[test]
fn cleanup_confirm_modal_fail_closed_on_drop() {
    use ruum_core::confirm::{
        CleanupConfirmDecision, CleanupConfirmItem, CleanupConfirmRequest,
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let resp_file = temp.path().join("decision.json");

    let request = CleanupConfirmRequest {
        root: PathBuf::from("/tmp"),
        title: "Test Cleanup".into(),
        summary: None,
        items: vec![CleanupConfirmItem {
            path: PathBuf::from("/tmp/test"),
            bytes: 50,
            mode: RemovalMode::Trash,
            reason: None,
            git_warning: None,
        }],
    };

    {
        let _modal = crate::state::CleanupConfirmModal::new(
            request,
            None,
            Some(resp_file.clone()),
        );
        // Dropped without explicit confirm or cancel
    }

    assert!(resp_file.exists());
    let content = std::fs::read_to_string(&resp_file).expect("read");
    let decision: CleanupConfirmDecision =
        serde_json::from_str(&content).expect("parse");
    assert_eq!(decision, CleanupConfirmDecision::Cancelled);
}

#[gpui_kit::test]
fn watchdog_and_settings_pages_rendering_and_modal_flow(
    cx: &mut TestAppContext,
) {
    use ruum_core::watchdog_config::ProposalItem;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);

    // 1. Navigate to Settings page
    update(&view, cx, |app, _| app.screen = Screen::Settings);
    draw(cx);
    assert!(cx.debug_bounds("settings-page").is_some());
    assert!(cx.debug_bounds("setting-enable-watchdog").is_some());
    assert!(cx.debug_bounds("setting-min-free-input").is_some());
    assert!(cx.debug_bounds("setting-poll-interval-input").is_some());
    assert!(cx.debug_bounds("setting-cooldown-input").is_some());
    assert!(cx.debug_bounds("setting-save-btn").is_some());

    // Focusing poll-interval or cooldown input marks the UI as editing.
    cx.update(|window, cx| {
        use gpui_kit::Focusable as _;
        view.update(cx, |app, cx| {
            let poll_input = app
                .watchdog_input_poll_interval
                .clone()
                .expect("poll interval input");
            window.focus(&poll_input.read(cx).focus_handle(cx), cx);
            assert!(app.is_editing(window, cx));

            let cooldown_input =
                app.watchdog_input_cooldown.clone().expect("cooldown input");
            window.focus(&cooldown_input.read(cx).focus_handle(cx), cx);
            assert!(app.is_editing(window, cx));

            window.focus(&app.focus, cx);
            assert!(!app.is_editing(window, cx));
        });
    });

    // Toggle watchdog setting, update poll interval & cooldown (minutes), save
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.watchdog_draft.enabled = true;
            if let Some(ref input) = app.watchdog_input_min_free {
                input.update(cx, |inp, cx| inp.set_value("35", window, cx));
            }
            if let Some(ref input) = app.watchdog_input_poll_interval {
                input.update(cx, |inp, cx| inp.set_value("10", window, cx));
            }
            if let Some(ref input) = app.watchdog_input_cooldown {
                input.update(cx, |inp, cx| inp.set_value("45", window, cx));
            }
            app.save_watchdog_settings(window, cx);
        });
    });
    assert!(read(&view, cx, |app| app.watchdog_config.enabled));
    assert_eq!(read(&view, cx, |app| app.watchdog_config.min_free_gb), 35);
    assert_eq!(
        read(&view, cx, |app| app.watchdog_config.poll_interval_secs),
        600
    );
    assert_eq!(
        read(&view, cx, |app| app.watchdog_config.cooldown_secs),
        2700
    );

    // Cooldown below 5 minutes clamps to MIN_COOLDOWN_SECS (300s) and echoes.
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            if let Some(ref input) = app.watchdog_input_poll_interval {
                input.update(cx, |inp, cx| inp.set_value("5", window, cx));
            }
            if let Some(ref input) = app.watchdog_input_cooldown {
                input.update(cx, |inp, cx| inp.set_value("2", window, cx));
            }
            app.save_watchdog_settings(window, cx);
        });
    });
    assert_eq!(
        read(&view, cx, |app| app.watchdog_config.poll_interval_secs),
        300
    );
    assert_eq!(
        read(&view, cx, |app| app.watchdog_config.cooldown_secs),
        ruum_core::watchdog_config::MIN_COOLDOWN_SECS
    );
    let cooldown_val =
        read(&view, cx, |app| app.watchdog_input_cooldown.clone())
            .expect("cooldown input");
    assert_eq!(
        cx.update(|_, cx| cooldown_val.read(cx).value().to_string()),
        "5"
    );

    // Zero minutes clamps to 300s, and invalid text aborts with Error toast.
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            if let Some(ref input) = app.watchdog_input_cooldown {
                input.update(cx, |inp, cx| inp.set_value("0", window, cx));
            }
            app.save_watchdog_settings(window, cx);
        });
    });
    assert_eq!(
        read(&view, cx, |app| app.watchdog_config.cooldown_secs),
        300
    );
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            if let Some(ref input) = app.watchdog_input_cooldown {
                input.update(cx, |inp, cx| inp.set_value("bad", window, cx));
            }
            app.save_watchdog_settings(window, cx);
        });
    });
    assert_eq!(
        read(&view, cx, |app| app.watchdog_config.cooldown_secs),
        300
    );
    assert_eq!(
        read(&view, cx, |app| app.notice.as_ref().map(|(_, s)| *s)),
        Some(gpui_omarchy::Status::Error)
    );

    // 2. Navigate to Watchdog page
    update(&view, cx, |app, _| app.screen = Screen::Watchdog);
    draw(cx);
    assert!(cx.debug_bounds("watchdog-page").is_some());
    assert!(cx.debug_bounds("watchdog-check-space").is_some());
    assert!(cx.debug_bounds("watchdog-ai-analyze").is_some());

    // 3. Inject mock proposal into watchdog state and open standalone window
    let target_file = temp.path().join("junk/blob.bin");
    update(&view, cx, |app, cx| {
        app.watchdog_state.last_proposal =
            Some(ruum_core::watchdog_config::CleanupProposal {
                timestamp_secs: 1000,
                summary: "AI recommends removing junk blobs".into(),
                items: vec![ProposalItem::new(
                    target_file.clone(),
                    200_000,
                    RemovalMode::Trash,
                    Some("junk test file".into()),
                    None,
                )],
            });
        app.open_last_proposal(cx);
    });
    draw(cx);

    // Standalone confirmation window opened as an independent second window.
    let windows = cx.windows();
    assert_eq!(windows.len(), 2, "expected standalone confirm window");
    let mut confirm_cx = VisualTestContext::from_window(windows[1], &cx.cx);
    draw(&mut confirm_cx);

    assert!(confirm_cx.debug_bounds("cleanup-confirm-window").is_some());
    assert!(confirm_cx.debug_bounds("cleanup-confirm-popup").is_some());
    assert!(confirm_cx.debug_bounds("confirm-cancel").is_some());
    assert!(confirm_cx.debug_bounds("confirm-clean").is_some());

    // Escape in standalone confirm window cancels and closes it without deleting.
    confirm_cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 1);
    assert!(target_file.exists());

    // In Watchdog screen, Escape returns to Explore
    press(cx, "escape");
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);
}

#[gpui_kit::test]
fn confirm_only_standalone_mode(cx: &mut TestAppContext) {
    use ruum_core::confirm::{
        CleanupConfirmDecision, CleanupConfirmItem, CleanupConfirmRequest,
    };

    cx.update(gpui_omarchy::init);
    let temp = tempfile::tempdir().expect("tempdir");
    let resp_file = temp.path().join("resp.json");

    let req = CleanupConfirmRequest {
        root: PathBuf::from("/tmp"),
        title: "Standalone Confirm".into(),
        summary: Some("Standalone mode".into()),
        items: vec![CleanupConfirmItem {
            path: PathBuf::from("/tmp/standalone.tmp"),
            bytes: 512,
            mode: RemovalMode::Trash,
            reason: None,
            git_warning: None,
        }],
    };

    let (view, cx) = cx.add_window_view(|_, cx| {
        crate::CleanupConfirmWindow::new(
            req,
            None,
            Some(resp_file.clone()),
            true,
            cx,
        )
    });
    let focus = view.read_with(cx, |win, _| win.focus.clone());
    cx.update(|window, cx| window.focus(&focus, cx));
    draw(cx);

    assert!(cx.debug_bounds("cleanup-confirm-popup").is_some());
    assert!(!resp_file.exists());

    // Press Enter to confirm
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(resp_file.exists());
    let content = std::fs::read_to_string(&resp_file).expect("read");
    let decision: CleanupConfirmDecision =
        serde_json::from_str(&content).expect("parse");
    match decision {
        CleanupConfirmDecision::Confirmed { selected_paths } => {
            assert_eq!(
                selected_paths,
                vec![PathBuf::from("/tmp/standalone.tmp")]
            );
        }
        CleanupConfirmDecision::Cancelled => panic!("expected confirmed"),
    }
}

#[gpui_kit::test]
fn open_confirm_window_toggles_confirms_and_fails_closed_on_close(
    cx: &mut TestAppContext,
) {
    use ruum_core::confirm::{
        CleanupConfirmDecision, CleanupConfirmItem, CleanupConfirmRequest,
    };
    use std::sync::mpsc::channel;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (_, cx) = view_over(temp.path(), cx);

    let path_a = temp.path().join("junk/blob.bin");
    let path_b = temp.path().join("keep/notes.txt");
    let request = CleanupConfirmRequest {
        root: temp.path().to_path_buf(),
        title: "IPC Confirm".into(),
        summary: Some("Select items".into()),
        items: vec![
            CleanupConfirmItem {
                path: path_a.clone(),
                bytes: 200_000,
                mode: RemovalMode::Trash,
                reason: Some("cache".into()),
                git_warning: None,
            },
            CleanupConfirmItem {
                path: path_b,
                bytes: 1_000,
                mode: RemovalMode::Trash,
                reason: Some("notes".into()),
                git_warning: None,
            },
        ],
    };

    // 1. Toggle item checkbox and click Confirm -> returns checked paths only.
    let (tx1, rx1) = channel();
    let handle1 = cx.update(|_, cx| {
        crate::open_confirm_window(request.clone(), Some(tx1), None, cx)
            .expect("confirm window opened")
    });
    assert_eq!(cx.windows().len(), 2);
    let mut confirm_cx = VisualTestContext::from_window(handle1.into(), &cx.cx);
    draw(&mut confirm_cx);

    let check_b = confirm_cx
        .debug_bounds("check-item-1")
        .expect("second item checkbox");
    confirm_cx.simulate_click(check_b.center(), gpui_kit::Modifiers::none());
    draw(&mut confirm_cx);

    let confirm_btn = confirm_cx
        .debug_bounds("confirm-clean")
        .expect("confirm button");
    confirm_cx
        .simulate_click(confirm_btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();

    assert_eq!(
        rx1.recv().expect("confirm decision"),
        CleanupConfirmDecision::Confirmed {
            selected_paths: vec![path_a],
        }
    );
    assert_eq!(cx.windows().len(), 1);

    // 2. Closing window without clicking Confirm -> fail-closed Cancelled.
    let (tx2, rx2) = channel();
    let handle2 = cx.update(|_, cx| {
        crate::open_confirm_window(request, Some(tx2), None, cx)
            .expect("second confirm window opened")
    });
    assert_eq!(cx.windows().len(), 2);
    let mut confirm_cx2 =
        VisualTestContext::from_window(handle2.into(), &cx.cx);
    draw(&mut confirm_cx2);
    confirm_cx2.update(|window, _| window.remove_window());
    cx.run_until_parked();

    assert_eq!(
        rx2.recv().expect("fail-closed cancel"),
        CleanupConfirmDecision::Cancelled
    );
    assert_eq!(cx.windows().len(), 1);
}

#[gpui_kit::test]
fn settings_feedback_model_selection_and_watchdog_refresh_ui(
    cx: &mut TestAppContext,
) {
    use ruum_core::ai_cli::{DiscoveredModel, ModelSpeedResult};
    use ruum_core::watchdog_config::AiCliProvider;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);

    // 1. Settings page: save feedback shows floating Toast (no inline notice).
    update(&view, cx, |app, _| {
        app.notice = None;
        app.screen = Screen::Settings;
    });
    draw(cx);
    assert!(cx.debug_bounds("toast-notification").is_none());
    assert!(cx.debug_bounds("setting-test-cli").is_some());
    assert!(cx.debug_bounds("setting-fetch-models").is_some());
    assert!(cx.debug_bounds("setting-cli-btn").is_some());
    assert!(cx.debug_bounds("setting-ai-auto-analyze").is_some());
    assert!(cx.debug_bounds("setting-extra-args-input").is_some());
    assert!(cx.debug_bounds("setting-save-status").is_none());
    assert!(cx.debug_bounds("settings-notice-line").is_none());

    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.save_watchdog_settings(window, cx);
        });
    });
    draw(cx);
    let toast_bounds = cx
        .debug_bounds("toast-notification")
        .expect("toast notification bounds");
    let viewport = cx.update(|window, _| window.viewport_size());
    assert!(
        toast_bounds.right() > viewport.width / 2.0,
        "expected toast on right half: {toast_bounds:?} vs {viewport:?}"
    );
    assert!(
        toast_bounds.bottom() > viewport.height / 2.0,
        "expected toast on bottom half: {toast_bounds:?} vs {viewport:?}"
    );
    assert!(cx.debug_bounds("setting-save-status").is_none());
    assert!(cx.debug_bounds("settings-notice-line").is_none());

    // Clicking the toast close button dismisses the notification.
    let close_btn = cx.debug_bounds("toast-close").expect("toast close button");
    cx.simulate_click(close_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(read(&view, cx, |app| app.notice.is_none()));
    assert!(cx.debug_bounds("toast-notification").is_none());

    // Inject discovered models & speed test output and verify UI + selection.
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.watchdog_models = vec![
                DiscoveredModel {
                    id: "gemini-4-hydrogen-high".into(),
                    label: "Gemini 4 Hydrogen (High)".into(),
                    provider: None,
                },
                DiscoveredModel {
                    id: "claude-sonnet-4-6".into(),
                    label: "Claude Sonnet 4.6".into(),
                    provider: None,
                },
            ];
            app.watchdog_speed_results = vec![ModelSpeedResult {
                model: "gemini-4-hydrogen-high".into(),
                elapsed_ms: 420,
                ok: true,
                detail: "OK".into(),
            }];
            app.select_watchdog_model("gemini-4-hydrogen-high", window, cx);
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("setting-model-list").is_some());
    assert!(cx.debug_bounds("setting-speed-all-btn").is_some());
    assert!(cx.debug_bounds("setting-model-row-0").is_some());
    assert!(cx.debug_bounds("setting-model-row-1").is_some());
    assert!(cx.debug_bounds("setting-model-speed-0").is_some());
    assert!(cx.debug_bounds("setting-model-speed-1").is_some());
    assert!(cx.debug_bounds("setting-model-select-0").is_some());
    assert!(cx.debug_bounds("setting-model-select-1").is_some());
    assert!(cx.debug_bounds("setting-speed-results").is_none());
    assert_eq!(
        read(&view, cx, |app| app.watchdog_draft.model.clone()),
        Some("gemini-4-hydrogen-high".to_string())
    );
    let input_model = read(&view, cx, |app| app.watchdog_input_model.clone())
        .expect("model input initialized");
    assert_eq!(
        cx.update(|_, cx| input_model.read(cx).value().to_string()),
        "gemini-4-hydrogen-high"
    );

    // Clicking the trailing Select button switches the selected model in draft.
    let select_second = cx
        .debug_bounds("setting-model-select-1")
        .expect("select second model");
    cx.simulate_click(select_second.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(
        read(&view, cx, |app| app.watchdog_draft.model.clone()),
        Some("claude-sonnet-4-6".to_string())
    );
    // Committed config only updates after Save Settings.
    assert_eq!(
        read(&view, cx, |app| app.watchdog_config.model.clone()),
        None
    );
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.save_watchdog_settings(window, cx);
        });
    });
    assert_eq!(
        read(&view, cx, |app| app.watchdog_config.model.clone()),
        Some("claude-sonnet-4-6".to_string())
    );

    // Switching providers clears stale models and speed history cleanly.
    update(&view, cx, |app, cx| {
        app.set_watchdog_provider(AiCliProvider::Pi, cx);
    });
    assert!(read(&view, cx, |app| app.watchdog_models.is_empty()));
    assert!(read(&view, cx, |app| app.watchdog_speed_results.is_empty()));
    assert!(read(&view, cx, |app| {
        app.watchdog_benchmarking_models.is_empty()
    }));

    // 2. Watchdog page: Check Disk Space Now produces a floating Toast.
    update(&view, cx, |app, _| {
        app.notice = None;
        app.screen = Screen::Watchdog;
    });
    draw(cx);
    assert!(cx.debug_bounds("watchdog-last-checked").is_some());
    assert!(cx.debug_bounds("watchdog-action-status").is_none());
    assert!(cx.debug_bounds("toast-notification").is_none());

    update(&view, cx, Ruum::refresh_watchdog_space);
    draw(cx);
    assert!(read(&view, cx, |app| {
        app.watchdog_space_checked_at.is_some()
    }));
    assert!(cx.debug_bounds("toast-notification").is_some());
    assert!(cx.debug_bounds("watchdog-action-status").is_none());
    assert!(cx.debug_bounds("watchdog-notice-line").is_none());
}

#[cfg(unix)]
#[gpui_kit::test]
fn watchdog_cli_discovery_speed_test_and_analysis_are_nonblocking(
    cx: &mut TestAppContext,
) {
    use ruum_core::watchdog_config::AiCliProvider;
    use std::os::unix::fs::PermissionsExt as _;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let fake_cli = temp.path().join("fake-agy");
    std::fs::write(
        &fake_cli,
        b"#!/bin/sh
case \"$1\" in
  --version)
    echo \"agy 1.5.8\"
    ;;
  models)
    echo \"gemini-4-hydrogen-high  Gemini 4 Hydrogen (High)\"
    echo \"claude-sonnet-4-6  Claude Sonnet 4.6\"
    ;;
  *)
    echo '{\"summary\":\"All clean\",\"items\":[]}'
    ;;
esac
",
    )
    .expect("write fake cli");
    std::fs::set_permissions(&fake_cli, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake cli");

    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1000.), px(1400.)));
    update(&view, cx, |app, _| {
        app.screen = Screen::Settings;
        app.watchdog_config.ai_provider = AiCliProvider::Agy;
        app.watchdog_draft.ai_provider = AiCliProvider::Agy;
        app.watchdog_config.custom_command =
            fake_cli.to_string_lossy().to_string();
    });

    // 1. Test connection only (without auto-fetching models).
    update(&view, cx, Ruum::test_watchdog_ai_cli);
    assert!(read(&view, cx, |app| app.watchdog_testing));
    cx.run_until_parked();
    assert!(!read(&view, cx, |app| app.watchdog_testing));
    assert!(read(&view, cx, |app| app.watchdog_models.is_empty()));
    assert_eq!(
        read(&view, cx, |app| {
            app.notice.as_ref().map(|(m, _)| m.clone())
        }),
        Some(format!("CLI ready at {}: agy 1.5.8", fake_cli.display()))
    );

    // 2. Fetch available models separately off the UI thread.
    update(&view, cx, Ruum::fetch_watchdog_models);
    assert!(read(&view, cx, |app| app.watchdog_fetching_models));
    cx.run_until_parked();
    assert!(!read(&view, cx, |app| app.watchdog_fetching_models));
    assert_eq!(read(&view, cx, |app| app.watchdog_models.len()), 2);

    // 3. Benchmark single model (verify in-flight spinning state, then click re-test).
    update(&view, cx, |app, cx| {
        app.benchmark_watchdog_model(Some("gemini-4-hydrogen-high".into()), cx);
    });
    assert!(read(&view, cx, |app| {
        app.watchdog_benchmarking_models
            .contains("gemini-4-hydrogen-high")
    }));
    draw(cx);
    cx.run_until_parked();
    assert!(read(&view, cx, |app| {
        app.watchdog_benchmarking_models.is_empty()
    }));
    let speeds = read(&view, cx, |app| app.watchdog_speed_results.clone());
    assert_eq!(speeds.len(), 1);
    assert!(speeds[0].ok);
    assert_eq!(speeds[0].model, "gemini-4-hydrogen-high");

    // Clicking the latency badge re-tests that model.
    update(&view, cx, |app, _| {
        app.watchdog_speed_results[0].elapsed_ms = 999_999;
    });
    draw(cx);
    let retest_btn_0 = cx
        .debug_bounds("setting-model-speed-0")
        .expect("model speed retest button 0");
    cx.simulate_click(retest_btn_0.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    assert!(read(&view, cx, |app| {
        app.watchdog_benchmarking_models.is_empty()
    }));
    assert!(read(&view, cx, |app| {
        app.watchdog_speed_results[0].elapsed_ms < 999_999
    }));

    // 4. Click header lightning icon to benchmark all models.
    draw(cx);
    let speed_all_btn = cx
        .debug_bounds("setting-speed-all-btn")
        .expect("benchmark all models button");
    cx.simulate_click(speed_all_btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    assert!(read(&view, cx, |app| {
        app.watchdog_benchmarking_models.is_empty()
    }));
    assert_eq!(read(&view, cx, |app| app.watchdog_speed_results.len()), 2);

    // 5. Watchdog Run AI Analysis Now returns immediately and settles via Toast.
    update(&view, cx, |app, cx| {
        app.screen = Screen::Watchdog;
        app.run_ai_analysis_now(cx);
    });
    assert!(read(&view, cx, |app| app.watchdog_analyzing));
    draw(cx);
    assert!(cx.debug_bounds("toast-notification").is_some());

    cx.run_until_parked();
    assert!(!read(&view, cx, |app| app.watchdog_analyzing));
    let (msg, expected) = read(&view, cx, |app| {
        (
            app.notice.as_ref().map(|(m, _)| m.clone()),
            crate::i18n::messages::ai_found_no_files(app.locale),
        )
    });
    assert_eq!(msg.as_deref(), Some(expected));
    assert_eq!(
        read(&view, cx, |app| app.watchdog_state.last_triggered_secs),
        None
    );
    assert_eq!(
        read(&view, cx, |app| {
            app.watchdog_state.last_triggered_threshold_gb
        }),
        None
    );
}

#[gpui_kit::test]
fn all_screens_render_across_all_locales(cx: &mut TestAppContext) {
    use crate::i18n::Locale;
    use ruum_core::confirm::{CleanupConfirmItem, CleanupConfirmRequest};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1400.), px(900.)));

    let target = temp.path().join("junk");
    update(&view, cx, |app, cx| {
        app.marks.toggle(ruum_core::removal::Target {
            path: target.clone(),
            bytes: 300_000,
            is_dir: true,
            hidden: false,
        });
        cx.notify();
    });

    for locale in crate::i18n::ALL_LOCALES {
        update(&view, cx, |app, cx| {
            app.set_locale(locale);
            app.navigate_rail(Screen::Explore, cx);
            app.show_help = false;
            app.confirm_open = false;
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("ruum-root").is_some());
        assert!(cx.debug_bounds("icon-rail").is_some());

        // Help overlay on Explore.
        update(&view, cx, |app, cx| {
            app.show_help = true;
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("ruum-root").is_some());

        // Review screen + permanent delete confirmation dialog.
        update(&view, cx, |app, cx| {
            app.show_help = false;
            app.screen = Screen::Review;
            app.removal_mode = RemovalMode::Permanent;
            app.confirm_open = true;
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("ruum-root").is_some());

        // Sources page (awaiting root -> localized window title).
        update(&view, cx, |app, cx| {
            app.confirm_open = false;
            app.screen = Screen::Explore;
            app.navigate_rail(Screen::Sources, cx);
            app.awaiting_root = true;
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("sources-page").is_some());
        assert!(cx.debug_bounds("choose-root-idle").is_some());
        let expected_choose_title =
            crate::i18n::messages::title_choose_source(locale);
        assert_eq!(
            read(&view, cx, |app| app.window_title().to_owned()),
            expected_choose_title
        );

        // Watchdog page.
        update(&view, cx, |app, cx| {
            app.awaiting_root = false;
            app.navigate_rail(Screen::Watchdog, cx);
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("watchdog-page").is_some());
        assert!(cx.debug_bounds("watchdog-check-space").is_some());
        assert!(cx.debug_bounds("watchdog-ai-analyze").is_some());

        // Settings page.
        update(&view, cx, |app, cx| {
            app.navigate_rail(Screen::Settings, cx);
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("settings-page").is_some());
        assert!(cx.debug_bounds("setting-save-btn").is_some());
    }

    // Standalone cleanup confirmation window across all locales.
    let req = CleanupConfirmRequest {
        root: temp.path().to_path_buf(),
        title: crate::i18n::messages::ai_proposal_title(Locale::Ja).into(),
        summary: None,
        items: vec![CleanupConfirmItem {
            path: target,
            bytes: 300_000,
            mode: RemovalMode::Trash,
            reason: Some("cache".into()),
            git_warning: Some("untracked files".into()),
        }],
    };
    let (confirm_view, confirm_cx) = cx.add_window_view(|_, cx| {
        crate::CleanupConfirmWindow::new(req, None, None, false, cx)
    });
    for locale in crate::i18n::ALL_LOCALES {
        confirm_view.update(confirm_cx, |win, cx| {
            win.set_locale(locale);
            cx.notify();
        });
        draw(confirm_cx);
        assert!(confirm_cx.debug_bounds("cleanup-confirm-popup").is_some());
        assert!(confirm_cx.debug_bounds("confirm-cancel").is_some());
        assert!(confirm_cx.debug_bounds("confirm-clean").is_some());
        assert!(
            !crate::i18n::messages::title_confirm_cleanup(locale).is_empty()
        );
    }
}

#[test]
fn initial_main_window_size_scales_and_clamps_to_minimum() {
    use gpui_kit::size;

    assert_eq!(
        crate::initial_main_window_size(Some(size(px(3840.), px(2160.)))),
        size(px(2304.), px(1296.))
    );
    assert_eq!(
        crate::initial_main_window_size(Some(size(px(1920.), px(1080.)))),
        size(px(1152.), px(648.))
    );
    assert_eq!(
        crate::initial_main_window_size(Some(size(px(1280.), px(720.)))),
        size(px(800.), px(600.))
    );
    assert_eq!(
        crate::initial_main_window_size(None),
        size(px(864.), px(600.))
    );
}

#[gpui_kit::test]
fn system_notification_posts_with_app_identity(cx: &TestAppContext) {
    use ruum_core::watchdog::NOTIF_TAG_LOW_SPACE;

    cx.update(|cx| {
        gpui_omarchy::init(cx);
        crate::install_notification_handler(cx);
        crate::post_app_notification(
            NOTIF_TAG_LOW_SPACE,
            "Low Disk Space",
            "Only 2 GB remaining",
            cx,
        );
    });

    assert_eq!(
        cx.app_identity(),
        Some(("io.github.tobi.ruum".into(), "ruum".into()))
    );
    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].tag.as_ref(), NOTIF_TAG_LOW_SPACE);
    assert_eq!(shown[0].title.as_ref(), "Low Disk Space");
    assert_eq!(shown[0].body.as_ref(), "Only 2 GB remaining");
}

#[gpui_kit::test]
fn clicking_low_space_notification_navigates_open_window_to_watchdog(
    cx: &mut TestAppContext,
) {
    use gpui_kit::SystemNotificationResponse;
    use ruum_core::watchdog::NOTIF_TAG_LOW_SPACE;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let tree = scan(temp.path(), options()).expect("scan the fixture");
    let root_path = temp.path().to_path_buf();
    let window = cx.add_window(move |_, cx| {
        Ruum::with_tree(root_path.clone(), tree, options(), 3, cx)
    });

    cx.update(|cx| {
        crate::install_notification_handler(cx);
        crate::install_test_runtime(
            temp.path().to_path_buf(),
            false,
            Some(window),
            cx,
        );
    });

    assert_eq!(
        window
            .read_with(cx, |app, _| app.screen)
            .expect("read explore screen"),
        Screen::Explore
    );

    cx.simulate_system_notification_response(SystemNotificationResponse {
        tag: NOTIF_TAG_LOW_SPACE.into(),
        action_id: None,
    });

    assert_eq!(
        window
            .read_with(cx, |app, _| app.screen)
            .expect("read watchdog screen"),
        Screen::Watchdog
    );
}

#[gpui_kit::test]
fn clicking_notification_reopens_closed_main_window(cx: &TestAppContext) {
    use gpui_kit::SystemNotificationResponse;
    use ruum_core::watchdog::{NOTIF_TAG_CLEANUP_DONE, NOTIF_TAG_LOW_SPACE};

    let temp = fixture();
    cx.update(|cx| {
        gpui_omarchy::init(cx);
        crate::install_notification_handler(cx);
        crate::install_test_runtime(temp.path().to_path_buf(), false, None, cx);
    });
    assert!(
        cx.read(|cx| {
            cx.global::<crate::AppRuntime>().main_window.is_none()
        })
    );

    cx.simulate_system_notification_response(SystemNotificationResponse {
        tag: NOTIF_TAG_LOW_SPACE.into(),
        action_id: None,
    });
    cx.run_until_parked();

    let reopened = cx.read(|cx| cx.global::<crate::AppRuntime>().main_window);
    let handle = reopened.expect("main window reopened for low-space alert");
    assert_eq!(
        handle
            .read_with(cx, |app, _| app.screen)
            .expect("read reopened watchdog screen"),
        Screen::Watchdog
    );

    // Simulate tray-resident mode again (window closed) and click cleanup-done
    // notification: should open window and preserve default Explore screen.
    cx.update(|cx| {
        crate::with_runtime_mut(cx, |rt| rt.main_window = None);
    });
    cx.simulate_system_notification_response(SystemNotificationResponse {
        tag: NOTIF_TAG_CLEANUP_DONE.into(),
        action_id: None,
    });
    cx.run_until_parked();

    let reopened_default =
        cx.read(|cx| cx.global::<crate::AppRuntime>().main_window);
    let default_handle = reopened_default
        .expect("main window reopened for cleanup notification");
    assert_eq!(
        default_handle
            .read_with(cx, |app, _| app.screen)
            .expect("read reopened default screen"),
        Screen::Explore
    );
}

#[gpui_kit::test]
fn review_auto_switches_to_permanent_for_trash_and_localizes_trash_block(
    cx: &mut TestAppContext,
) {
    use ruum_core::trash_adapter::{
        ALREADY_IN_TRASH_REASON, TrashEnv, TrashOs,
    };

    cx.update(gpui_omarchy::init);
    let temp = tempfile::TempDir::new().expect("tempdir");
    let trash_file = temp.path().join(".Trash/old.bin");
    let keep_file = temp.path().join("keep/notes.txt");
    std::fs::create_dir_all(trash_file.parent().expect("parent"))
        .expect("mkdir trash");
    std::fs::create_dir_all(keep_file.parent().expect("parent"))
        .expect("mkdir keep");
    std::fs::write(&trash_file, vec![b't'; 8192]).expect("write trash");
    std::fs::write(&keep_file, b"keep").expect("write keep");

    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.trash_env = Some(TrashEnv {
            home: Some(temp.path().to_path_buf()),
            xdg_data_home: None,
            uid: Some(501),
        });
        app.trash_os = Some(TrashOs::MacOs);
        app.removal_mode = RemovalMode::Trash;
        app.marks.toggle(ruum_core::removal::Target {
            path: temp.path().join(".Trash"),
            bytes: 8192,
            is_dir: true,
            hidden: true,
        });
        cx.notify();
    });
    draw(cx);

    // Entering Review when only trash targets are marked auto-switches to Permanent.
    press(cx, "c");
    read(&view, cx, |app| {
        assert_eq!(app.screen, Screen::Review);
        assert_eq!(app.removal_mode, RemovalMode::Permanent);
        let plan = app.plan();
        assert_eq!(plan.targets.len(), 1);
        assert!(plan.blocked.is_empty());
    });

    // Switching back to Trash mode blocks .Trash with ALREADY_IN_TRASH_REASON,
    // disables commit, and renders localized reason cleanly.
    press(cx, "m");
    update(&view, cx, |app, cx| {
        app.locale = crate::i18n::Locale::Zh;
        cx.notify();
    });
    draw(cx);
    read(&view, cx, |app| {
        assert_eq!(app.removal_mode, RemovalMode::Trash);
        let plan = app.plan();
        assert!(plan.is_empty());
        assert_eq!(plan.blocked.len(), 1);
        assert_eq!(plan.blocked[0].reason, ALREADY_IN_TRASH_REASON);
        assert_ne!(
            crate::i18n::review::blocked_reason(
                &plan.blocked[0].reason,
                app.locale
            ),
            ALREADY_IN_TRASH_REASON
        );
    });

    // Enter in Trash mode must not open confirm dialog or start running.
    press(cx, "enter");
    read(&view, cx, |app| {
        assert_eq!(app.screen, Screen::Review);
        assert!(!app.confirm_open);
    });

    // Switching back to Permanent still requires the mandatory confirm dialog.
    press(cx, "p");
    press(cx, "enter");
    read(&view, cx, |app| {
        assert_eq!(app.screen, Screen::Review);
        assert!(app.confirm_open);
    });
    press(cx, "escape");
    read(&view, cx, |app| {
        assert!(!app.confirm_open);
    });

    // If a non-trash target is also marked, open_review preserves Trash mode.
    update(&view, cx, |app, cx| {
        app.screen = Screen::Explore;
        app.removal_mode = RemovalMode::Trash;
        app.marks.toggle(ruum_core::removal::Target {
            path: temp.path().join("keep"),
            bytes: 4,
            is_dir: true,
            hidden: false,
        });
        app.open_review();
        cx.notify();
    });
    draw(cx);
    read(&view, cx, |app| {
        assert_eq!(app.screen, Screen::Review);
        assert_eq!(app.removal_mode, RemovalMode::Trash);
    });

    // Unmark "keep" so only .Trash remains, then confirm permanent deletion
    // end-to-end and verify .Trash contents are emptied while .Trash stays.
    #[cfg(unix)]
    {
        update(&view, cx, |app, cx| {
            app.unmark(&temp.path().join("keep"), cx);
            app.screen = Screen::Explore;
            app.removal_mode = RemovalMode::Trash;
            app.open_review();
            cx.notify();
        });
        draw(cx);
        read(&view, cx, |app| {
            assert_eq!(app.removal_mode, RemovalMode::Permanent);
        });

        press(cx, "enter");
        assert!(read(&view, cx, |app| app.confirm_open));

        press(cx, "enter");
        assert!(!read(&view, cx, |app| app.confirm_open));
        assert_eq!(read(&view, cx, |app| app.screen), Screen::Running);

        let epoch = read(&view, cx, |app| app.run_epoch);
        let mut finished = false;
        for _ in 0..400 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            finished = update(&view, cx, |app, cx| {
                app.poll_removal_once(epoch, cx);
                app.screen == Screen::Done
            });
            if finished {
                break;
            }
        }
        assert!(finished, "permanent trash empty finished");
        draw(cx);

        let summary = read(&view, cx, |app| app.run_summary.clone());
        assert_eq!(summary.removed, 1, "{summary:?}");
        assert_eq!(summary.failed, 0, "{summary:?}");
        assert!(!trash_file.exists(), "item inside .Trash must be removed");
        assert!(
            temp.path().join(".Trash").is_dir(),
            ".Trash container directory must remain intact"
        );
        assert!(keep_file.exists(), "unmarked files must stay untouched");
    }
}

#[gpui_kit::test]
fn settings_default_360_echo_and_10_field_round_trip_without_host_pollution(
    cx: &mut TestAppContext,
) {
    use ruum_core::watchdog_config::{
        AiCliProvider, DEFAULT_COOLDOWN_SECS, WatchdogConfig, WatchdogState,
        load_config, load_state,
    };

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_path = temp.path().join("watchdog.json");
    let state_path = temp.path().join("watchdog-state.json");
    let (view, cx) = view_over(temp.path(), cx);

    // Under cfg!(test), paths are None so host config is never touched.
    read(&view, cx, |app| {
        assert!(app.watchdog_config_path.is_none());
        assert!(app.watchdog_state_path.is_none());
        assert_eq!(app.watchdog_config.cooldown_secs, DEFAULT_COOLDOWN_SECS);
    });

    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(cx);

    // Defaults echo "20", "5", "360", and extra_args/auto-analyze are visible.
    assert!(cx.debug_bounds("setting-ai-auto-analyze").is_some());
    assert!(cx.debug_bounds("setting-extra-args-input").is_some());
    assert!(cx.debug_bounds("setting-cli-btn").is_some());
    assert!(cx.debug_bounds("setting-custom-cmd-input").is_none());

    let val_of = |cx: &mut Window,
                  sel: fn(
        &Ruum,
    ) -> &Option<
        Entity<gpui_kit::base::input::InputState>,
    >| {
        let inp = read(&view, cx, |app| sel(app).clone()).expect("input");
        cx.update(|_, cx| inp.read(cx).value().to_string())
    };
    assert_eq!(val_of(cx, |a| &a.watchdog_input_min_free), "20");
    assert_eq!(val_of(cx, |a| &a.watchdog_input_poll_interval), "5");
    assert_eq!(val_of(cx, |a| &a.watchdog_input_cooldown), "360");

    // Inject temp config/state paths and pre-seed cooldown state to verify reset.
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.watchdog_config_path = Some(cfg_path.clone());
            app.watchdog_state_path = Some(state_path.clone());
            app.watchdog_state = WatchdogState {
                last_checked_secs: Some(500),
                last_triggered_secs: Some(400),
                last_triggered_threshold_gb: Some(20),
                last_proposal: None,
            };
            app.watchdog_draft.enabled = true;
            app.watchdog_draft.background_service = true;
            app.watchdog_draft.ai_auto_analyze = false;
            app.set_watchdog_provider(AiCliProvider::Custom, cx);
            app.watchdog_input_min_free.as_ref().unwrap().update(
                cx,
                |i, cx| {
                    i.set_value("42", window, cx);
                },
            );
            app.watchdog_input_poll_interval
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("15", window, cx));
            app.watchdog_input_cooldown.as_ref().unwrap().update(
                cx,
                |i, cx| {
                    i.set_value("120", window, cx);
                },
            );
            app.watchdog_input_custom_cmd.as_ref().unwrap().update(
                cx,
                |i, cx| {
                    i.set_value("/opt/bin/custom-ai", window, cx);
                },
            );
            app.watchdog_input_extra_args
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("--fast --json", window, cx));
            app.watchdog_input_model
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| {
                    i.set_value("gpt-5-codex", window, cx);
                });
            app.save_watchdog_settings(window, cx);
        });
    });

    let expected = WatchdogConfig {
        enabled: true,
        background_service: true,
        min_free_gb: 42,
        poll_interval_secs: 900,
        cooldown_secs: 7200,
        ai_auto_analyze: false,
        ai_provider: AiCliProvider::Custom,
        custom_command: "/opt/bin/custom-ai".into(),
        extra_args: "--fast --json".into(),
        model: Some("gpt-5-codex".into()),
    };
    assert_eq!(load_config(&cfg_path).expect("saved cfg"), expected);
    let saved_state = load_state(&state_path).expect("saved state");
    assert_eq!(saved_state.last_triggered_secs, None);
    assert_eq!(saved_state.last_triggered_threshold_gb, None);

    // Reload into a fresh Ruum instance and confirm all 10 fields echo back.
    let (fresh_view, fresh_cx) = view_over(temp.path(), &mut cx.cx);
    update(&fresh_view, fresh_cx, |app, cx| {
        app.watchdog_config_path = Some(cfg_path.clone());
        app.watchdog_state_path = Some(state_path.clone());
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(fresh_cx);
    assert!(fresh_cx.debug_bounds("setting-custom-cmd-input").is_some());
    read(&fresh_view, fresh_cx, |app| {
        assert_eq!(app.watchdog_config, expected);
        assert!(app.watchdog_draft.enabled);
        assert!(app.watchdog_draft.background_service);
        assert!(!app.watchdog_draft.ai_auto_analyze);
        assert_eq!(app.watchdog_draft.ai_provider, AiCliProvider::Custom);
        assert_eq!(app.watchdog_draft.model.as_deref(), Some("gpt-5-codex"));
    });
    let fresh_val = |cx: &mut Window,
                     sel: fn(
        &Ruum,
    ) -> &Option<
        Entity<gpui_kit::base::input::InputState>,
    >| {
        let inp = read(&fresh_view, cx, |app| sel(app).clone()).expect("input");
        cx.update(|_, cx| inp.read(cx).value().to_string())
    };
    assert_eq!(fresh_val(fresh_cx, |a| &a.watchdog_input_min_free), "42");
    assert_eq!(
        fresh_val(fresh_cx, |a| &a.watchdog_input_poll_interval),
        "15"
    );
    assert_eq!(fresh_val(fresh_cx, |a| &a.watchdog_input_cooldown), "120");
    assert_eq!(
        fresh_val(fresh_cx, |a| &a.watchdog_input_custom_cmd),
        "/opt/bin/custom-ai"
    );
    assert_eq!(
        fresh_val(fresh_cx, |a| &a.watchdog_input_extra_args),
        "--fast --json"
    );
    assert_eq!(
        fresh_val(fresh_cx, |a| &a.watchdog_input_model),
        "gpt-5-codex"
    );
}

#[gpui_kit::test]
fn settings_draft_isolation_validation_and_clamped_echo(
    cx: &mut TestAppContext,
) {
    use ruum_core::watchdog_config::{AiCliProvider, load_config};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_path = temp.path().join("watchdog.json");
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.watchdog_config_path = Some(cfg_path.clone());
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(cx);

    // Mutate draft + inputs without saving -> committed config unchanged.
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.watchdog_draft.enabled = true;
            app.watchdog_draft.ai_auto_analyze = false;
            app.set_watchdog_provider(AiCliProvider::Codex, cx);
            app.watchdog_input_min_free
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("99", window, cx));
        });
    });
    read(&view, cx, |app| {
        assert!(!app.watchdog_config.enabled);
        assert!(app.watchdog_config.ai_auto_analyze);
        assert_eq!(app.watchdog_config.ai_provider, AiCliProvider::Agy);
    });

    // Re-entering Settings resets unsaved draft and input edits from config.
    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Explore, cx);
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(cx);
    read(&view, cx, |app| {
        assert!(!app.watchdog_draft.enabled);
        assert!(app.watchdog_draft.ai_auto_analyze);
        assert_eq!(app.watchdog_draft.ai_provider, AiCliProvider::Agy);
    });
    let min_free_inp = read(&view, cx, |a| a.watchdog_input_min_free.clone())
        .expect("min free input");
    assert_eq!(
        cx.update(|_, cx| min_free_inp.read(cx).value().to_string()),
        "20"
    );

    // Invalid numeric inputs abort Save with Status::Error and write nothing.
    for bad in ["", "abc", "-3"] {
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.watchdog_draft.enabled = true;
                app.watchdog_input_min_free
                    .as_ref()
                    .unwrap()
                    .update(cx, |i, cx| i.set_value(bad, window, cx));
                app.save_watchdog_settings(window, cx);
            });
        });
        assert!(!read(&view, cx, |a| a.watchdog_config.enabled));
        assert!(!cfg_path.exists());
        assert_eq!(
            read(&view, cx, |a| a.notice.as_ref().map(|(_, s)| *s)),
            Some(gpui_omarchy::Status::Error)
        );
    }

    // Clamped inputs ("0", "0", "2") normalize to 1 GB, 1 min, 5 min and echo.
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.watchdog_input_min_free
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("0", window, cx));
            app.watchdog_input_poll_interval
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("0", window, cx));
            app.watchdog_input_cooldown
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("2", window, cx));
            app.save_watchdog_settings(window, cx);
        });
    });
    let saved = load_config(&cfg_path).expect("saved clamped");
    assert_eq!(saved.min_free_gb, 1);
    assert_eq!(saved.poll_interval_secs, 60);
    assert_eq!(saved.cooldown_secs, 300);

    let poll_inp = read(&view, cx, |a| a.watchdog_input_poll_interval.clone())
        .expect("poll input");
    let cool_inp = read(&view, cx, |a| a.watchdog_input_cooldown.clone())
        .expect("cooldown input");
    assert_eq!(
        cx.update(|_, cx| min_free_inp.read(cx).value().to_string()),
        "1"
    );
    assert_eq!(
        cx.update(|_, cx| poll_inp.read(cx).value().to_string()),
        "1"
    );
    assert_eq!(
        cx.update(|_, cx| cool_inp.read(cx).value().to_string()),
        "5"
    );
}

#[cfg(unix)]
#[gpui_kit::test]
fn settings_cli_install_card_and_menu_action_work_without_host_mutation(
    cx: &mut TestAppContext,
) {
    use ruum_core::cli_install::{
        CliInstallEnv, CliInstallOs, CliInstallStatus,
    };

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let fake_exe = temp.path().join("bin/ruum");
    std::fs::create_dir_all(fake_exe.parent().unwrap()).expect("mkdir bin");
    std::fs::write(&fake_exe, b"#!/bin/sh\n").expect("write fake exe");
    let fake_home = temp.path().join("home");
    let local_bin = fake_home.join(".local/bin");
    std::fs::create_dir_all(&fake_home).expect("mkdir home");
    let env = CliInstallEnv {
        home: Some(fake_home),
        local_appdata: None,
        path_var: Some(local_bin.to_string_lossy().into_owned()),
        usr_local_bin: temp.path().join("usr_local_bin"),
    };

    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.cli_install_env = Some(env);
        app.cli_install_os = Some(CliInstallOs::Linux);
        app.cli_install_exe = Some(fake_exe.clone());
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(cx);

    // Click Install CLI Command button -> installs symlink into temp home.
    let btn = cx.debug_bounds("setting-cli-btn").expect("cli button");
    cx.simulate_click(btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    draw(cx);

    let link = local_bin.join("ruum");
    assert!(link.is_symlink(), "expected symlink at {}", link.display());
    assert_eq!(
        read(&view, cx, |a| a.cli_install_status.clone()),
        Some(CliInstallStatus::Installed {
            path: link.clone(),
            in_path: true,
        })
    );
    assert_eq!(
        read(&view, cx, |a| a.notice.as_ref().map(|(_, s)| *s)),
        Some(gpui_omarchy::Status::Success)
    );

    // Click again -> uninstalls symlink cleanly.
    let btn = cx.debug_bounds("setting-cli-btn").expect("cli button");
    cx.simulate_click(btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    draw(cx);
    assert!(!link.exists() && std::fs::symlink_metadata(&link).is_err());
    assert_eq!(
        read(&view, cx, |a| a.cli_install_status.clone()),
        Some(CliInstallStatus::NotInstalled)
    );

    // Dispatching InstallCliCommand action navigates to Settings and installs.
    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Explore, cx);
    });
    draw(cx);
    cx.dispatch_action(crate::app_menu::InstallCliCommand);
    cx.run_until_parked();
    draw(cx);
    assert_eq!(read(&view, cx, |a| a.screen), Screen::Settings);
    assert!(link.is_symlink());
}

#[gpui_kit::test]
fn watchdog_page_handles_none_space_saturating_mul_and_syncs_checked_at(
    cx: &mut TestAppContext,
) {
    use ruum_core::watchdog_config::{WatchdogState, save_state};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let state_path = temp.path().join("watchdog-state.json");
    save_state(
        &state_path,
        &WatchdogState {
            last_checked_secs: Some(1_234_567),
            last_triggered_secs: None,
            last_triggered_threshold_gb: None,
            last_proposal: None,
        },
    )
    .expect("save state");

    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.watchdog_state_path = Some(state_path);
        app.watchdog_home_space = None;
        app.space = None;
        app.watchdog_config.min_free_gb = u64::MAX;
        app.reload_watchdog_state();
        app.navigate_rail(Screen::Watchdog, cx);
    });
    draw(cx);

    read(&view, cx, |app| {
        assert_eq!(app.watchdog_space_checked_at, Some(1_234_567));
    });
    assert!(cx.debug_bounds("watchdog-page").is_some());
    assert!(cx.debug_bounds("watchdog-status-value").is_some());
}

#[gpui_kit::test]
fn settings_model_list_scroll_is_isolated_from_outer_page(
    cx: &mut TestAppContext,
) {
    use gpui_kit::{
        Modifiers, ScrollDelta, ScrollWheelEvent, TouchPhase, point,
    };
    use ruum_core::ai_cli::DiscoveredModel;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);

    // Resize to a height where `setting-model-list` is visible in the
    // viewport while the outer `settings-page` still overflows vertically.
    cx.simulate_resize(gpui_kit::size(px(1000.), px(1050.)));
    update(&view, cx, |app, _| {
        app.screen = Screen::Settings;
        app.watchdog_models = (0..20)
            .map(|i| DiscoveredModel {
                id: format!("model-{i:02}"),
                label: format!("Model {i:02}"),
                provider: None,
            })
            .collect();
    });
    draw(cx);

    let list_before = cx
        .debug_bounds("setting-model-list")
        .expect("setting-model-list bounds");
    let row0_before = cx
        .debug_bounds("setting-model-row-0")
        .expect("setting-model-row-0 bounds");
    let test_cli_before = cx
        .debug_bounds("setting-test-cli")
        .expect("setting-test-cli bounds");
    let save_btn_before = cx
        .debug_bounds("setting-save-btn")
        .expect("setting-save-btn bounds");
    assert!(
        list_before.center().y < px(1050.),
        "expected setting-model-list center inside viewport: {list_before:?}"
    );
    assert!(
        save_btn_before.bottom() > px(1050.),
        "expected settings-page to overflow viewport: {save_btn_before:?}"
    );
    let initial_rel_y = row0_before.origin.y - list_before.origin.y;

    // 1. Scroll inside `setting-model-list`: inner list must scroll while outer
    //    `settings-page` controls remain stationary.
    cx.simulate_event(ScrollWheelEvent {
        position: list_before.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    draw(cx);

    let list_after_inner = cx
        .debug_bounds("setting-model-list")
        .expect("setting-model-list bounds after inner scroll");
    let row0_after_inner = cx
        .debug_bounds("setting-model-row-0")
        .expect("setting-model-row-0 bounds after inner scroll");
    let test_cli_after_inner = cx
        .debug_bounds("setting-test-cli")
        .expect("setting-test-cli bounds after inner scroll");
    let save_btn_after_inner = cx
        .debug_bounds("setting-save-btn")
        .expect("setting-save-btn bounds after inner scroll");

    let rel_after_inner = row0_after_inner.origin.y - list_after_inner.origin.y;
    assert!(
        rel_after_inner < initial_rel_y,
        "expected inner list to scroll upward: before={initial_rel_y:?}, \
         after={rel_after_inner:?}"
    );
    assert_eq!(
        test_cli_after_inner.origin.y, test_cli_before.origin.y,
        "outer setting-test-cli should not move when scrolling inside \
         setting-model-list"
    );
    assert_eq!(
        save_btn_after_inner.origin.y, save_btn_before.origin.y,
        "outer setting-save-btn should not move when scrolling inside \
         setting-model-list"
    );

    // 2. Scroll outside `setting-model-list` (over `setting-test-cli`): outer
    //    `settings-page` must scroll while inner list offset stays unchanged.
    cx.simulate_event(ScrollWheelEvent {
        position: test_cli_after_inner.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    draw(cx);

    let list_after_outer = cx
        .debug_bounds("setting-model-list")
        .expect("setting-model-list bounds after outer scroll");
    let row0_after_outer = cx
        .debug_bounds("setting-model-row-0")
        .expect("setting-model-row-0 bounds after outer scroll");
    let test_cli_after_outer = cx
        .debug_bounds("setting-test-cli")
        .expect("setting-test-cli bounds after outer scroll");
    let save_btn_after_outer = cx
        .debug_bounds("setting-save-btn")
        .expect("setting-save-btn bounds after outer scroll");

    assert!(
        test_cli_after_outer.origin.y < test_cli_after_inner.origin.y,
        "expected outer setting-test-cli to shift upward: before={:?}, \
         after={:?}",
        test_cli_after_inner.origin.y,
        test_cli_after_outer.origin.y
    );
    assert!(
        save_btn_after_outer.origin.y < save_btn_after_inner.origin.y,
        "expected outer setting-save-btn to shift upward: before={:?}, \
         after={:?}",
        save_btn_after_inner.origin.y,
        save_btn_after_outer.origin.y
    );
    let rel_after_outer = row0_after_outer.origin.y - list_after_outer.origin.y;
    assert_eq!(
        rel_after_outer, rel_after_inner,
        "inner model list scroll offset should stay unchanged when scrolling \
         outer settings-page"
    );
}
