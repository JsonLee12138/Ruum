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
    assert!(cx.debug_bounds("volume-picker-modal").is_some());
    assert!(cx.debug_bounds("modal-source-path").is_some());
    assert!(cx.debug_bounds("source-path").is_none());
    assert!(cx.debug_bounds("ssh-profiles-card").is_none());
    read(&view, cx, |app| {
        assert_eq!(app.screen, Screen::Explore);
        assert!(app.volumes_open);
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
    let input = cx.debug_bounds("modal-source-path").expect("path editor");
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
    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Remote, cx);
        app.open_add_ssh_form(cx);
    });
    draw(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.ssh_input_alias
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("prod", window, cx));
            app.ssh_input_host
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("invalid host", window, cx));
            app.ssh_input_username
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("alice", window, cx));
            app.save_ssh_profile_form(cx);
        });
    });
    read(&view, cx, |app| {
        assert!(app.ssh_form_error.is_some());
        assert!(app.ssh_profiles.is_empty());
        assert!(app.remote.is_none());
        assert!(app.scan.is_none() && app.remote_scan.is_none());
        assert_eq!(app.scan_epoch, 0);
    });
    press(cx, "escape");
    assert!(!read(&view, cx, |app| app.ssh_form_open));
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Remote);
    press(cx, "escape");
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);
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
        app.choose_source_root(target.path().to_path_buf(), cx);
        assert!(app.remote.is_none() && app.remote_identity.is_none());
        assert!(app.marks.is_empty());
        assert!(app.tree.is_none());
        assert_eq!(app.active_host_tab, crate::state::HostTabKey::Local);
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
            .debug_bounds("volume-picker-modal")
            .expect("unified volume picker modal");
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
    update(&view, cx, |app, cx| app.navigate_rail(Screen::Remote, cx));
    cx.run_until_parked();
    update(&view, cx, Ruum::open_add_ssh_form);
    draw(cx);
    let before = read(&view, cx, |app| app.scan_epoch);
    let input = cx.debug_bounds("ssh-form-host").unwrap();
    cx.simulate_click(input.center(), gpui_kit::Modifiers::none());
    cx.simulate_input("new-host-not-connected");
    read(&view, cx, |app| {
        assert!(app.remote.is_none());
        assert_eq!(app.scan_epoch, before);
        assert!(app.tree.is_some());
    });
    press(cx, "escape");
    assert!(!read(&view, cx, |app| app.ssh_form_open));
    draw(cx);
    press(cx, "escape");
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);
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
fn stale_local_discovery_cannot_overwrite_closed_picker_state(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let (view, cx) =
        cx.add_window_view(|_, cx| Ruum::choose(options(), 3, None, cx));
    update(&view, cx, Ruum::load_local_volumes);
    update(&view, cx, Ruum::close_volumes);
    cx.run_until_parked();
    draw(cx);
    read(&view, cx, |app| {
        assert!(!app.volumes_open);
        assert!(!app.volumes_loading);
        assert!(app.remote.is_none());
    });
}

#[gpui_kit::test]
fn local_mounts_never_scan_nonexistent_remote_only_paths(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.host_tabs.push(crate::state::ExploreHostTab {
            key: crate::state::HostTabKey::Ssh("srv-1".into()),
            label: "srv-1".into(),
            session: None,
        });
        app.switch_host_tab(crate::state::HostTabKey::Ssh("srv-1".into()), cx);
        app.volumes_open = true;
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
        let before_epoch = app.scan_epoch;
        app.choose_volume(cx);
        assert!(app.picker_error.is_some());
        assert_eq!(app.scan_epoch, before_epoch);
        assert_eq!(
            app.active_host_tab,
            crate::state::HostTabKey::Ssh("srv-1".into())
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

    // The picker lists volumes and draws them in a modal over Explore without
    // jumping to Screen::Remote; Escape leaves the scan where it was.
    press(cx, "v");
    assert!(read(&view, cx, |app| app.volumes_open));
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);
    draw(cx);
    assert!(cx.debug_bounds("volume-picker-modal").is_some());
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
    update(&view, cx, |app, cx| {
        app.open_volumes(cx);
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
    let modal = cx
        .debug_bounds("volume-picker-modal")
        .expect("volume picker modal is drawn");
    let rows = cx
        .debug_bounds("modal-volume-rows")
        .expect("the modal rows are drawn");
    assert!(modal.size.width > px(200.), "{modal:?}");
    assert!(rows.size.width > px(200.), "{rows:?}");
    assert!(
        cx.debug_bounds("modal-source-cancel").is_some(),
        "modal picker has a Cancel button"
    );
    assert!(
        modal.origin.x > px(0.) && modal.right() < viewport.width,
        "modal picker is centred horizontally: {modal:?} in {viewport:?}"
    );

    // Clicking outside the modal card dismisses it and returns to Explore.
    let outside = cx.debug_bounds("ruum-root").expect("ruum-root");
    cx.simulate_click(
        gpui_kit::point(outside.origin.x + px(8.), outside.origin.y + px(8.)),
        gpui_kit::Modifiers::none(),
    );
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);
    assert!(!read(&view, cx, |app| app.volumes_open));
}

#[gpui_kit::test]
fn remote_page_renders_only_ssh_card_and_ignores_volume_keys(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    let initial_root = read(&view, cx, |app| app.root_path.clone());
    let initial_epoch = read(&view, cx, |app| app.scan_epoch);

    // Opening Remote while volume modal is open closes the modal and never
    // starts local volume discovery (`volumes_loading` stays false).
    update(&view, cx, |app, cx| {
        app.volumes_open = true;
        app.volumes_loading = true;
        app.volumes = vec![Volume {
            point: temp.path().to_path_buf(),
            device: Some("/dev/disk".into()),
            space: None,
        }];
        app.volume_highlight = 0;
        app.navigate_rail(Screen::Remote, cx);
    });
    draw(cx);

    read(&view, cx, |app| {
        assert_eq!(app.screen, Screen::Remote);
        assert!(!app.volumes_open);
        assert!(!app.volumes_loading);
    });

    let viewport = cx.update(|window, _| window.viewport_size());
    let page = cx
        .debug_bounds("remote-page")
        .expect("remote full page is drawn");
    let rail = crate::ui::size::ICON_RAIL.to_pixels(px(16.));
    assert!(
        (page.origin.x - rail).abs() < px(2.),
        "remote page starts next to icon rail: {page:?}"
    );
    assert!(
        (page.size.width - (viewport.width - rail)).abs() < px(2.),
        "remote page fills remaining width: {page:?} in {viewport:?}"
    );
    assert!(cx.debug_bounds("remote-content").is_some());
    assert!(cx.debug_bounds("ssh-profiles-card").is_some());
    assert!(cx.debug_bounds("volume-rows").is_none());
    assert!(cx.debug_bounds("source-path").is_none());
    assert!(cx.debug_bounds("source-scan-path").is_none());
    assert!(cx.debug_bounds("source-scan").is_none());

    // Clicking empty space on Remote does NOT dismiss it.
    cx.simulate_click(
        gpui_kit::point(page.right() - px(8.), page.origin.y + px(8.)),
        gpui_kit::Modifiers::none(),
    );
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Remote);

    // Stale volume-list keys (`enter`, `up`, `down`, `j`, `k`) are ignored.
    for key in ["down", "j", "up", "k", "enter"] {
        press(cx, key);
        read(&view, cx, |app| {
            assert_eq!(app.screen, Screen::Remote);
            assert_eq!(app.volume_highlight, 0);
            assert_eq!(app.root_path, initial_root);
            assert_eq!(app.scan_epoch, initial_epoch);
        });
    }

    // Escape returns to Explore.
    press(cx, "escape");
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);
}

#[gpui_kit::test]
fn breadcrumb_and_ssh_browse_open_unified_directory_modal(
    cx: &mut TestAppContext,
) {
    use gpui_kit::Modifiers;
    use ruum_core::storage::{AuthMode, SshProfile};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let other = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1200.), px(800.)));
    draw(cx);

    // 1. Clicking the scan-root breadcrumb opens the unified modal in-place on
    //    Screen::Explore (never jumping to Screen::Remote).
    let root_crumb =
        cx.debug_bounds("crumb-scan-root").expect("crumb-scan-root");
    cx.simulate_click(root_crumb.center(), Modifiers::none());
    draw(cx);
    assert!(read(&view, cx, |app| app.volumes_open));
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);
    assert!(cx.debug_bounds("volume-picker-modal").is_some());

    // Clicking outside the modal card dismisses it.
    let outside = cx.debug_bounds("ruum-root").expect("ruum-root");
    cx.simulate_click(
        gpui_kit::point(outside.origin.x + px(8.), outside.origin.y + px(8.)),
        Modifiers::none(),
    );
    draw(cx);
    assert!(!read(&view, cx, |app| app.volumes_open));

    // 2. Clicking the trail directory-switcher icon opens the modal too, and
    //    submitting a local directory switches root_path in Explore.
    let switch_btn = cx
        .debug_bounds("trail-switch-root")
        .expect("trail-switch-root");
    cx.simulate_click(switch_btn.center(), Modifiers::none());
    cx.run_until_parked();
    draw(cx);
    assert!(read(&view, cx, |app| app.volumes_open));
    update(&view, cx, |app, _| {
        app.volumes_loading = false;
        app.volumes_error = None;
        app.volumes = vec![Volume {
            point: other.path().to_path_buf(),
            device: None,
            space: None,
        }];
        app.volume_highlight = 0;
    });
    draw(cx);
    let scan_btn = cx
        .debug_bounds("modal-source-scan")
        .expect("modal-source-scan");
    cx.simulate_click(scan_btn.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(!read(&view, cx, |app| app.volumes_open));
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);
    assert_eq!(
        read(&view, cx, |app| app.root_path.clone()),
        dunce::canonicalize(other.path()).unwrap()
    );
    finish_scan(&view, cx);

    // 3. Remote SSH modal mode validates remote paths (~ / absolute) without
    //    touching local FS, and SSH cards expose `ssh-browse-0`.
    let prof = SshProfile {
        id: "srv-modal-1".into(),
        alias: "edge-box".into(),
        host: "10.9.9.9".into(),
        port: 22,
        username: "ops".into(),
        auth_mode: AuthMode::PrivateKey,
        default_path: None,
        created_at_secs: 1,
        updated_at_secs: 1,
        last_total_bytes: None,
        last_available_bytes: None,
        last_scanned_secs: None,
        pending_script_path: None,
        pending_script_items: None,
        pending_reclaim_bytes: None,
        pending_created_secs: None,
    };
    update(&view, cx, |app, cx| {
        app.ssh_profiles = vec![prof.clone()];
        app.host_tabs.push(crate::state::ExploreHostTab {
            key: crate::state::HostTabKey::Ssh(prof.id.clone()),
            label: prof.alias.clone(),
            session: None,
        });
        app.navigate_rail(Screen::Remote, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("ssh-browse-0").is_some());

    // Simulate remote modal opened for the active SSH tab.
    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Explore, cx);
        app.switch_host_tab(
            crate::state::HostTabKey::Ssh("srv-modal-1".into()),
            cx,
        );
        app.volumes_open = true;
        app.volume_picker_remote_profile_id = Some("srv-modal-1".into());
        app.volumes_loading = false;
        app.volumes = vec![Volume {
            point: PathBuf::from("/home/ops"),
            device: Some("Home directory".into()),
            space: None,
        }];
        app.choose_remote_root("srv-modal-1", Path::new("relative"), cx);
        assert!(app.picker_error.is_some());
        assert!(app.volumes_open);
    });
    draw(cx);
    assert!(cx.debug_bounds("modal-picker-error").is_some());
    assert!(cx.debug_bounds("volume-picker-modal").is_some());
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
fn restart_arguments_parse_back_to_the_same_root() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().join("a folder");
    std::fs::create_dir(&root).expect("mkdir");
    let root = dunce::canonicalize(&root).expect("canonical root");
    let args = crate::state::restart_args(&root);
    let parsed = crate::parse_gui_args(args.into_iter()).expect("parses");
    assert_eq!(parsed.root, root);
}

#[gpui_kit::test]
fn power_efficiency_menu_saves_without_discarding_the_tree(
    cx: &mut TestAppContext,
) {
    use crate::power::PowerEfficiency as Power;
    use gpui_kit::Modifiers;
    use ruum_core::storage::StorageDb;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let config = tempfile::tempdir().expect("config");
    let path = config.path().join("ruum.db");
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(900.), px(600.)));
    let mut locales = vec![read(&view, cx, |app| app.locale)];
    if !locales.contains(&crate::i18n::Locale::En) {
        locales.push(crate::i18n::Locale::En);
    }
    let epoch = read(&view, cx, |app| app.scan_epoch);
    for locale in locales {
        update(&view, cx, |app, cx| {
            app.set_locale(locale);
            app.db_path = Some(path.clone());
            // Enough CPUs that every preset is offered, whatever runs the test.
            app.cpu_threads = 18;
            app.power_menu = None;
            app.power_hover = false;
            app.notice = None;
            cx.notify();
        });
        for preset in Power::ALL {
            draw(cx);
            let button = cx.debug_bounds("power-efficiency").expect("control");
            cx.simulate_click(button.center(), Modifiers::none());
            draw(cx);
            let choice = cx.debug_bounds(preset.key()).expect("preset");
            cx.simulate_click(choice.center(), Modifiers::none());
            draw(cx);
            let saved = StorageDb::open(&path)
                .expect("open db")
                .load_app_settings()
                .expect("saved");
            assert_eq!(saved.power_preset, preset.key());
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
            app.db_path = Some(config.path().to_owned());
            app.set_power_efficiency(Power::Miser, cx);
        });
        assert!(read(&view, cx, |app| {
            let label = Power::Miser.label_in(app.cpu_threads, app.locale);
            let prefix = crate::i18n::messages::power_save_failed(
                &label, "", app.locale,
            );
            // Compare without the OS-specific trailing error text.
            let head = prefix.trim_end();
            app.notice
                .as_ref()
                .is_some_and(|(message, _)| message.starts_with(head))
        }));
    }
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
    let mut locales = vec![read(&view, cx, |app| app.locale)];
    if !locales.contains(&crate::i18n::Locale::En) {
        locales.push(crate::i18n::Locale::En);
    }
    for locale in locales {
        cx.simulate_mouse_move(
            gpui_kit::point(px(0.), px(0.)),
            None,
            Modifiers::none(),
        );
        update(&view, cx, |app, cx| {
            app.set_locale(locale);
            app.db_path = Some(config.path().join("ruum.db"));
            // Aggressive and Drain My Battery would both run four workers.
            app.cpu_threads = 4;
            app.power_choice = Some(Power::Balanced);
            app.power_menu = None;
            app.power_hover = false;
            app.notice = None;
            cx.notify();
        });
        draw(cx);
        let toggle = cx.debug_bounds("power-efficiency").expect("control");
        cx.simulate_mouse_move(toggle.center(), None, Modifiers::none());
        cx.simulate_click(toggle.center(), Modifiers::none());
        draw(cx);
        assert_eq!(read(&view, cx, |app| app.power_menu), Some(1));

        // The pointer takes the highlight, and the check stays on what is
        // saved.
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
}

#[gpui_kit::test]
fn top_bar_wraps_cleanly_at_min_width_across_all_locales(
    cx: &mut TestAppContext,
) {
    use crate::state::{ExploreHostTab, ExploreSessionSnapshot, HostTabKey};
    use gpui_kit::Modifiers;
    use ruum_core::docker_adapter::{DockerProbeStatus, DockerUsageSummary};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);

    let mut avail_snap = ExploreSessionSnapshot::default();
    avail_snap.root_path = PathBuf::from("/srv/docker");
    avail_snap.scan_root = PathBuf::from("/srv/docker");
    avail_snap.tree =
        Some(std::sync::Arc::new(scan(temp.path(), options()).unwrap()));
    avail_snap.remote = Some("prod-docker".into());
    avail_snap.remote_platform = "linux".into();
    avail_snap.docker = DockerProbeStatus::Available(DockerUsageSummary {
        build_cache_reclaimable_bytes: 500_000_000,
        detailed: true,
        ..DockerUsageSummary::default()
    });
    update(&view, cx, |app, cx| {
        app.cpu_threads = 18;
        app.host_tabs.push(ExploreHostTab {
            key: HostTabKey::Ssh("srv-docker".into()),
            label: "prod-docker".into(),
            session: Some(avail_snap),
        });
        app.switch_host_tab(HostTabKey::Ssh("srv-docker".into()), cx);
    });

    let assert_inside =
        |name: &str, bounds: Bounds<Pixels>, width: f32, height: f32| {
            assert!(
                bounds.origin.x >= px(0.)
                    && bounds.right() <= px(width)
                    && bounds.origin.y >= px(0.)
                    && bounds.bottom() <= px(height),
                "{name} out of {width}x{height} bounds: {bounds:?}"
            );
        };

    cx.simulate_resize(gpui_kit::size(px(900.), px(600.)));
    for locale in crate::i18n::ALL_LOCALES {
        update(&view, cx, |app, cx| {
            app.set_locale(locale);
            app.power_menu = None;
            cx.notify();
        });
        draw(cx);
        let switch_bounds = cx
            .debug_bounds("trail-switch-root")
            .expect("trail-switch-root");
        let docker_bounds = cx
            .debug_bounds("remote-docker-btn")
            .expect("remote-docker-btn");
        let power_bounds = cx
            .debug_bounds("power-efficiency")
            .expect("power-efficiency");
        assert_inside("trail-switch-root", switch_bounds, 900., 600.);
        assert_inside("remote-docker-btn", docker_bounds, 900., 600.);
        assert_inside("power-efficiency", power_bounds, 900., 600.);

        cx.simulate_mouse_move(power_bounds.center(), None, Modifiers::none());
        cx.simulate_click(power_bounds.center(), Modifiers::none());
        draw(cx);
        assert!(
            read(&view, cx, |app| app.power_menu.is_some()),
            "power-menu failed to open in {locale:?}"
        );
        let menu_bounds = cx.debug_bounds("power-menu").expect("power-menu");
        assert_inside("power-menu", menu_bounds, 900., 600.);
    }

    // At the default 1440x900 width, the top bar stays on a single row.
    cx.simulate_resize(gpui_kit::size(px(1440.), px(900.)));
    update(&view, cx, |app, cx| {
        app.set_locale(crate::i18n::Locale::En);
        app.power_menu = None;
        cx.notify();
    });
    draw(cx);
    let switch_bounds = cx
        .debug_bounds("trail-switch-root")
        .expect("trail-switch-root");
    let docker_bounds = cx
        .debug_bounds("remote-docker-btn")
        .expect("remote-docker-btn");
    let power_bounds = cx
        .debug_bounds("power-efficiency")
        .expect("power-efficiency");
    assert_inside("trail-switch-root", switch_bounds, 1440., 900.);
    assert_inside("remote-docker-btn", docker_bounds, 1440., 900.);
    assert_inside("power-efficiency", power_bounds, 1440., 900.);
    assert!(
        (switch_bounds.center().y - power_bounds.center().y).abs() < px(10.),
        "top bar wrapped at 1440x900: switch={switch_bounds:?}, \
         power={power_bounds:?}"
    );
}

#[gpui_kit::test]
fn icon_rail_switches_pages_and_settings_shortcut(cx: &mut TestAppContext) {
    use ruum_core::docker_adapter::{DockerProbeStatus, DockerUsageSummary};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    // Initial page is Explore; `rail-docker` stays hidden while local Docker
    // is not Available.
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);
    assert!(cx.debug_bounds("icon-rail").is_some());
    assert!(cx.debug_bounds("rail-explore").is_some());
    assert!(cx.debug_bounds("rail-docker").is_none());
    assert!(cx.debug_bounds("rail-remote").is_some());
    assert!(cx.debug_bounds("rail-watchdog").is_some());
    assert!(cx.debug_bounds("rail-settings").is_some());

    // OpenDocker action is ignored when local Docker daemon is unavailable.
    cx.dispatch_action(crate::app_menu::OpenDocker);
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);

    // Once local Docker daemon is Available, `rail-docker` appears and opens
    // `Screen::Docker`.
    update(&view, cx, |app, cx| {
        app.apply_local_docker_status(DockerProbeStatus::Available(
            DockerUsageSummary::default(),
        ));
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("rail-docker").is_some());

    // Explore -> Docker -> Escape returns to Explore
    let docker_btn = cx.debug_bounds("rail-docker").expect("rail-docker");
    cx.simulate_click(docker_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Docker);
    assert!(cx.debug_bounds("docker-page").is_some());
    press(cx, "escape");
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);

    // Explore -> Remote
    let remote_btn = cx.debug_bounds("rail-remote").expect("rail-remote");
    cx.simulate_click(remote_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Remote);
    assert!(cx.debug_bounds("remote-page").is_some());

    // Remote -> Watchdog
    let watchdog_btn = cx.debug_bounds("rail-watchdog").expect("rail-watchdog");
    cx.simulate_click(watchdog_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Watchdog);
    assert!(cx.debug_bounds("watchdog-page").is_some());

    // Watchdog -> Remote (bidirectional click)
    let remote_btn = cx.debug_bounds("rail-remote").expect("rail-remote");
    cx.simulate_click(remote_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Remote);
    assert!(cx.debug_bounds("remote-page").is_some());

    // Remote -> Settings
    let settings_btn = cx.debug_bounds("rail-settings").expect("rail-settings");
    cx.simulate_click(settings_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Settings);
    assert!(cx.debug_bounds("settings-page").is_some());

    // Settings -> Remote (bidirectional click)
    let remote_btn = cx.debug_bounds("rail-remote").expect("rail-remote");
    cx.simulate_click(remote_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Remote);
    assert!(cx.debug_bounds("remote-page").is_some());

    // Remote -> Explore
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
    cx.dispatch_action(crate::app_menu::OpenDocker);
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Docker);

    // Dropping local Docker daemon while on Screen::Docker falls back to
    // Explore and hides `rail-docker`.
    update(&view, cx, |app, cx| {
        app.apply_local_docker_status(DockerProbeStatus::DaemonStopped);
        cx.notify();
    });
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Explore);
    assert!(cx.debug_bounds("rail-docker").is_none());

    cx.dispatch_action(crate::app_menu::OpenWatchdog);
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Watchdog);

    cx.dispatch_action(crate::app_menu::OpenRemote);
    draw(cx);
    assert_eq!(read(&view, cx, |app| app.screen), Screen::Remote);

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
        docker_items: Vec::new(),
        docker_permission_hint: None,
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
            selected_docker_targets: Vec::new(),
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
        docker_items: Vec::new(),
        docker_permission_hint: None,
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

    // 1. Navigate to Settings page (Watchdog category)
    update(&view, cx, |app, cx| {
        app.screen = Screen::Settings;
        app.set_settings_category(crate::state::SettingsCategory::Watchdog, cx);
    });
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

    // 2. Navigate to Watchdog page (auto-refreshes space without action card)
    update(&view, cx, |app, cx| {
        app.watchdog_home_space = None;
        app.watchdog_space_checked_at = None;
        app.navigate_rail(Screen::Watchdog, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("watchdog-page").is_some());
    assert!(cx.debug_bounds("watchdog-check-space").is_none());
    assert!(cx.debug_bounds("watchdog-ai-analyze").is_none());
    read(&view, cx, |app| {
        assert!(app.watchdog_home_space.is_some());
        assert!(app.watchdog_space_checked_at.is_some());
    });

    // 3. Inject mock proposal into watchdog state and open standalone window
    let target_file = temp.path().join("junk/blob.bin");
    update(&view, cx, |app, cx| {
        app.ai_proposal = Some(ruum_core::watchdog_config::CleanupProposal {
            timestamp_secs: 1000,
            summary: "AI recommends removing junk blobs".into(),
            items: vec![ProposalItem::new(
                target_file.clone(),
                200_000,
                RemovalMode::Trash,
                Some("junk test file".into()),
                None,
            )],
            docker_targets: Vec::new(),
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
        docker_items: Vec::new(),
        docker_permission_hint: None,
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
        CleanupConfirmDecision::Confirmed { selected_paths, .. } => {
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
        docker_items: Vec::new(),
        docker_permission_hint: None,
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
            selected_docker_targets: Vec::new(),
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
    update(&view, cx, |app, cx| {
        app.notice = None;
        app.screen = Screen::Settings;
        app.set_settings_category(crate::state::SettingsCategory::Ai, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("toast-notification").is_none());
    assert!(cx.debug_bounds("setting-test-cli").is_some());
    assert!(cx.debug_bounds("setting-fetch-models").is_some());
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
    assert_eq!(read(&view, cx, |app| app.ai_config.model.clone()), None);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.save_watchdog_settings(window, cx);
        });
    });
    assert_eq!(
        read(&view, cx, |app| app.ai_config.model.clone()),
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

    // 2. Navigating to Watchdog auto-refreshes space and timestamp without
    //    manual action buttons or inline status noise.
    update(&view, cx, |app, cx| {
        app.notice = None;
        app.watchdog_home_space = None;
        app.watchdog_space_checked_at = None;
        app.navigate_rail(Screen::Watchdog, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("watchdog-last-checked").is_some());
    assert!(cx.debug_bounds("watchdog-check-space").is_none());
    assert!(cx.debug_bounds("watchdog-ai-analyze").is_none());
    assert!(cx.debug_bounds("watchdog-action-status").is_none());
    assert!(cx.debug_bounds("watchdog-notice-line").is_none());
    assert!(cx.debug_bounds("toast-notification").is_none());
    assert!(read(&view, cx, |app| {
        app.watchdog_home_space.is_some()
            && app.watchdog_space_checked_at.is_some()
    }));
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
    update(&view, cx, |app, cx| {
        app.screen = Screen::Settings;
        app.set_settings_category(crate::state::SettingsCategory::Ai, cx);
        app.ai_config.provider = AiCliProvider::Agy;
        app.watchdog_draft.ai_provider = AiCliProvider::Agy;
        app.ai_config.custom_command = fake_cli.to_string_lossy().to_string();
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

        // Explore (awaiting root -> localized window title + idle guide).
        update(&view, cx, |app, cx| {
            app.confirm_open = false;
            app.screen = Screen::Explore;
            app.navigate_rail(Screen::Explore, cx);
            app.awaiting_root = true;
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("choose-root-idle").is_some());
        let expected_choose_title =
            crate::i18n::messages::title_choose_source(locale);
        assert_eq!(
            read(&view, cx, |app| app.window_title().to_owned()),
            expected_choose_title
        );

        // Remote page.
        update(&view, cx, |app, cx| {
            app.awaiting_root = false;
            app.navigate_rail(Screen::Remote, cx);
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("remote-page").is_some());
        assert!(cx.debug_bounds("remote-content").is_some());
        assert!(cx.debug_bounds("ssh-profiles-card").is_some());

        // Watchdog page.
        update(&view, cx, |app, cx| {
            app.awaiting_root = false;
            app.navigate_rail(Screen::Watchdog, cx);
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("watchdog-page").is_some());
        assert!(cx.debug_bounds("watchdog-check-space").is_none());
        assert!(cx.debug_bounds("watchdog-ai-analyze").is_none());

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
        docker_items: Vec::new(),
        docker_permission_hint: None,
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
        size(px(1440.), px(900.))
    );
    assert_eq!(
        crate::initial_main_window_size(Some(size(px(1920.), px(1080.)))),
        size(px(1440.), px(900.))
    );
    assert_eq!(
        crate::initial_main_window_size(Some(size(px(1280.), px(720.)))),
        size(px(1280.), px(720.))
    );
    assert_eq!(
        crate::initial_main_window_size(Some(size(px(800.), px(500.)))),
        size(px(900.), px(600.))
    );
    assert_eq!(
        crate::initial_main_window_size(None),
        size(px(1440.), px(900.))
    );
    assert_eq!(crate::DEFAULT_MAIN_WINDOW_SIZE, size(px(1440.), px(900.)));
    assert_eq!(crate::MIN_MAIN_WINDOW_SIZE, size(px(900.), px(600.)));
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
    use ruum_core::storage::{AiConfig, StorageDb};
    use ruum_core::watchdog_config::AiCliProvider;
    use ruum_watchdog::{DEFAULT_COOLDOWN_SECS, WatchdogConfig, WatchdogState};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let db_path = temp.path().join("ruum.db");
    let (view, cx) = view_over(temp.path(), cx);

    // Under cfg!(test), db_path is None so host config is never touched.
    read(&view, cx, |app| {
        assert!(app.db_path.is_none());
        assert_eq!(app.watchdog_config.cooldown_secs, DEFAULT_COOLDOWN_SECS);
    });

    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Settings, cx);
        app.set_settings_category(crate::state::SettingsCategory::Ai, cx);
    });
    draw(cx);

    // Defaults echo "20", "5", "360", and extra_args/auto-analyze are visible.
    assert!(cx.debug_bounds("setting-ai-auto-analyze").is_some());
    assert!(cx.debug_bounds("setting-extra-args-input").is_some());
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

    // Pre-seed cooldown state in temp SQLite DB to verify threshold reset.
    {
        let db = StorageDb::open(&db_path).expect("open temp db");
        let svc = db.watchdog().expect("watchdog service");
        svc.save_state(&WatchdogState {
            last_checked_secs: Some(500),
            last_triggered_secs: Some(400),
            last_triggered_threshold_gb: Some(20),
        })
        .expect("seed state");
    }

    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.db_path = Some(db_path.clone());
            app.reload_watchdog_state();
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

    let db = StorageDb::open(&db_path).expect("reopen temp db");
    let svc = db.watchdog().expect("watchdog service");
    let saved_wd = svc.load_config().expect("saved wd cfg");
    assert!(saved_wd.updated_at_secs > 0);
    let expected_wd = WatchdogConfig {
        enabled: true,
        background_service: true,
        min_free_gb: 42,
        poll_interval_secs: 900,
        cooldown_secs: 7200,
        updated_at_secs: saved_wd.updated_at_secs,
    };
    assert_eq!(saved_wd, expected_wd);
    let saved_state = svc.load_state().expect("saved state");
    assert_eq!(saved_state.last_triggered_secs, None);
    assert_eq!(saved_state.last_triggered_threshold_gb, None);
    let saved_ai = db.load_ai_config().expect("saved ai cfg");
    assert!(saved_ai.updated_at_secs > 0);
    let expected_ai = AiConfig {
        provider: AiCliProvider::Custom,
        custom_command: "/opt/bin/custom-ai".into(),
        extra_args: "--fast --json".into(),
        model: Some("gpt-5-codex".into()),
        auto_analyze_on_alert: false,
        updated_at_secs: saved_ai.updated_at_secs,
        ..AiConfig::default()
    };
    assert_eq!(saved_ai, expected_ai);

    // Reload into a fresh Ruum instance and confirm all fields echo back.
    let (fresh_view, fresh_cx) = view_over(temp.path(), &mut cx.cx);
    update(&fresh_view, fresh_cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.reload_watchdog_config_and_prepare_settings();
        app.navigate_rail(Screen::Settings, cx);
        app.set_settings_category(crate::state::SettingsCategory::Ai, cx);
    });
    draw(fresh_cx);
    assert!(fresh_cx.debug_bounds("setting-custom-cmd-input").is_some());
    read(&fresh_view, fresh_cx, |app| {
        assert!(app.watchdog_config.updated_at_secs > 0);
        assert!(app.ai_config.updated_at_secs > 0);
        assert_eq!(app.watchdog_config, expected_wd);
        assert_eq!(app.ai_config, expected_ai);
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
    use ruum_core::storage::StorageDb;
    use ruum_core::watchdog_config::AiCliProvider;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let db_path = temp.path().join("ruum.db");
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
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
        assert!(app.ai_config.auto_analyze_on_alert);
        assert_eq!(app.ai_config.provider, AiCliProvider::Agy);
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
        assert!(!db_path.exists());
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
    let saved = StorageDb::open(&db_path)
        .expect("open temp db")
        .watchdog()
        .expect("watchdog svc")
        .load_config()
        .expect("saved clamped");
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
        app.set_settings_category(crate::state::SettingsCategory::Cli, cx);
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
    use ruum_core::storage::StorageDb;
    use ruum_watchdog::WatchdogState;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let db_path = temp.path().join("ruum.db");
    {
        let db = StorageDb::open(&db_path).expect("open temp db");
        db.watchdog()
            .expect("watchdog svc")
            .save_state(&WatchdogState {
                last_checked_secs: Some(1_234_567),
                last_triggered_secs: None,
                last_triggered_threshold_gb: None,
            })
            .expect("save state");
    }

    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path);
        app.watchdog_home_space = None;
        app.space = None;
        app.watchdog_config.min_free_gb = u64::MAX;
        app.reload_watchdog_state();
        assert_eq!(app.watchdog_space_checked_at, Some(1_234_567));
        app.navigate_rail(Screen::Watchdog, cx);
    });
    read(&view, cx, |app| {
        assert!(app.watchdog_home_space.is_some());
        assert!(
            app.watchdog_space_checked_at
                .is_some_and(|ts| ts >= 1_234_567)
        );
    });
    update(&view, cx, |app, cx| {
        app.watchdog_home_space = None;
        app.space = None;
        cx.notify();
    });
    draw(cx);
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
    // viewport while `settings-content` still overflows vertically.
    cx.simulate_resize(gpui_kit::size(px(1000.), px(660.)));
    update(&view, cx, |app, cx| {
        app.screen = Screen::Settings;
        app.set_settings_category(crate::state::SettingsCategory::Ai, cx);
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
        list_before.center().y < save_btn_before.origin.y,
        "expected setting-model-list center above pinned save bar: \
         list={list_before:?}, save={save_btn_before:?}"
    );
    assert!(
        list_before.bottom() > save_btn_before.origin.y,
        "expected settings-content to overflow above pinned save bar: \
         list={list_before:?}, save={save_btn_before:?}"
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
    assert_eq!(
        save_btn_after_outer.origin.y, save_btn_after_inner.origin.y,
        "expected bottom setting-save-btn bar to remain pinned"
    );
    let rel_after_outer = row0_after_outer.origin.y - list_after_outer.origin.y;
    assert_eq!(
        rel_after_outer, rel_after_inner,
        "inner model list scroll offset should stay unchanged when scrolling \
         outer settings-page"
    );
}

#[derive(Debug, Default)]
struct CountingRootSecret {
    loads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    secret: std::sync::Mutex<Option<[u8; 32]>>,
}

impl CountingRootSecret {
    fn new(seed: [u8; 32]) -> Self {
        Self {
            loads: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            secret: std::sync::Mutex::new(Some(seed)),
        }
    }

    fn load_count(&self) -> usize {
        self.loads.load(std::sync::atomic::Ordering::SeqCst)
    }
}

struct CountingRootSecretHandle {
    loads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    seed: [u8; 32],
}

impl ruum_core::credentials::RootSecretStore for CountingRootSecretHandle {
    fn get_or_create_root_secret(
        &self,
    ) -> Result<zeroize::Zeroizing<[u8; 32]>, String> {
        self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(zeroize::Zeroizing::new(self.seed))
    }
}

impl CountingRootSecret {
    fn store_for(
        &self,
        enc_path: &Path,
    ) -> ruum_core::credentials::LazyCredentialStore {
        let seed = self.secret.lock().expect("lock").unwrap_or([0x42; 32]);
        let handle = CountingRootSecretHandle {
            loads: std::sync::Arc::clone(&self.loads),
            seed,
        };
        ruum_core::credentials::LazyCredentialStore::new(
            enc_path.to_path_buf(),
            std::sync::Arc::new(handle),
        )
    }
}

#[gpui_kit::test]
fn default_explore_startup_does_not_touch_keychain_or_legacy_text_files(
    cx: &mut TestAppContext,
) {
    use ruum_core::storage::{AuthMode, SshProfile, StorageDb};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let enc_path = cfg_dir.path().join("device-credentials.enc");
    {
        let db = StorageDb::open(&db_path).expect("open db");
        db.add_ssh_profile(&SshProfile {
            id: "prof-1".into(),
            alias: "prod-box".into(),
            host: "10.0.0.5".into(),
            port: 22,
            username: "deploy".into(),
            auth_mode: AuthMode::PrivateKey,
            default_path: Some("/var/log".into()),
            created_at_secs: 100,
            updated_at_secs: 100,
            last_total_bytes: Some(1_000_000_000),
            last_available_bytes: Some(250_000_000),
            last_scanned_secs: Some(900),
            pending_script_path: None,
            pending_script_items: None,
            pending_reclaim_bytes: None,
            pending_created_secs: None,
        })
        .expect("seed ssh profile");
    }

    let counter = CountingRootSecret::new([0x11; 32]);
    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, _| {
        app.db_path = Some(db_path.clone());
        app.cred_store = Some(counter.store_for(&enc_path));
        app.reload_ssh_profiles();
        app.reload_watchdog_config_and_prepare_settings();
    });
    draw(cx);

    read(&view, cx, |app| {
        assert_eq!(app.screen, Screen::Explore);
        assert!(app.tree.is_some());
        assert_eq!(app.ssh_profiles.len(), 1);
    });
    assert_eq!(counter.load_count(), 0);
    assert!(!cfg_dir.path().join("watchdog.json").exists());
    assert!(!cfg_dir.path().join("watchdog-state.json").exists());
    assert!(!cfg_dir.path().join("power-efficiency").exists());

    // Rendering Remote SSH cards from SQLite also must not touch the keychain.
    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Remote, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("ssh-profiles-card").is_some());
    assert!(cx.debug_bounds("ssh-card-0").is_some());
    assert_eq!(counter.load_count(), 0);
}

#[gpui_kit::test]
fn remote_ssh_card_crud_sqlite_persistence_and_on_demand_connect_entry(
    cx: &mut TestAppContext,
) {
    use gpui_kit::Modifiers;
    use ruum_core::storage::{AuthMode, SshProfile, StorageDb};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let enc_path = cfg_dir.path().join("device-credentials.enc");

    let counter = CountingRootSecret::new([0x2A; 32]);
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1100.), px(900.)));
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.cred_store = Some(counter.store_for(&enc_path));
        app.navigate_rail(Screen::Remote, cx);
    });
    draw(cx);
    assert_eq!(counter.load_count(), 0);

    // 1. Open Add Server form and fill Password-authenticated SSH server.
    let add_btn = cx.debug_bounds("ssh-add-btn").expect("ssh-add-btn");
    cx.simulate_click(add_btn.center(), Modifiers::none());
    draw(cx);
    assert!(cx.debug_bounds("ssh-form").is_some());

    let auth_pw_btn = cx
        .debug_bounds("ssh-form-auth-password")
        .expect("auth password btn");
    cx.simulate_click(auth_pw_btn.center(), Modifiers::none());
    draw(cx);

    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.ssh_input_alias
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("lab-server", window, cx));
            app.ssh_input_host
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("127.0.0.1", window, cx));
            app.ssh_input_port
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("2222", window, cx));
            app.ssh_input_username
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("tester", window, cx));
            app.ssh_input_default_path
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("/tmp", window, cx));
            app.ssh_input_password
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("s3cr3t", window, cx));
            app.save_ssh_profile_form(cx);
        });
    });
    draw(cx);

    assert!(cx.debug_bounds("ssh-form").is_none());
    assert!(cx.debug_bounds("ssh-card-0").is_some());
    assert!(enc_path.exists());
    // Saving encrypted credentials called RootSecretStore once.
    assert_eq!(counter.load_count(), 1);

    let profiles = StorageDb::open(&db_path)
        .expect("open db")
        .list_ssh_profiles()
        .expect("list profiles");
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].alias, "lab-server");
    assert_eq!(profiles[0].port, 2222);
    assert_eq!(profiles[0].auth_mode, AuthMode::Password);
    assert_eq!(profiles[0].default_path.as_deref(), Some("/tmp"));

    // 2. Edit alias without touching secret field -> keeps saved secret.
    let edit_btn = cx.debug_bounds("ssh-edit-0").expect("ssh-edit-0");
    cx.simulate_click(edit_btn.center(), Modifiers::none());
    draw(cx);
    assert!(cx.debug_bounds("ssh-form").is_some());
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.ssh_input_alias
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("lab-renamed", window, cx));
            app.save_ssh_profile_form(cx);
        });
    });
    draw(cx);

    let updated = StorageDb::open(&db_path)
        .expect("open db")
        .list_ssh_profiles()
        .expect("list profiles");
    assert_eq!(updated[0].alias, "lab-renamed");
    assert_eq!(updated[0].auth_mode, AuthMode::Password);

    // 2b. Switching auth_mode without entering a matching secret is rejected
    //     so SQLite auth_mode never diverges from encrypted store variant.
    let edit_btn2 = cx.debug_bounds("ssh-edit-0").expect("ssh-edit-0");
    cx.simulate_click(edit_btn2.center(), Modifiers::none());
    draw(cx);
    let key_mode_btn = cx
        .debug_bounds("ssh-form-auth-key")
        .expect("ssh-form-auth-key");
    cx.simulate_click(key_mode_btn.center(), Modifiers::none());
    draw(cx);
    update(&view, cx, |app, cx| {
        app.save_ssh_profile_form(cx);
    });
    draw(cx);
    assert!(read(&view, cx, |app| {
        app.ssh_form_open && app.ssh_form_error.is_some()
    }));
    let still_pw = StorageDb::open(&db_path)
        .expect("open db")
        .list_ssh_profiles()
        .expect("list profiles");
    assert_eq!(still_pw[0].auth_mode, AuthMode::Password);
    update(&view, cx, Ruum::close_ssh_form);

    // 3. Fresh Ruum reloads SQLite profiles with 0 keychain calls until Connect.
    let counter2 = CountingRootSecret::new([0x2A; 32]);
    let (fresh_view, fresh_cx) = view_over(temp.path(), &mut cx.cx);
    update(&fresh_view, fresh_cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.cred_store = Some(counter2.store_for(&enc_path));
        app.reload_ssh_profiles();
        app.navigate_rail(Screen::Remote, cx);
    });
    draw(fresh_cx);
    assert_eq!(counter2.load_count(), 0);

    // 4. Clicking Connect & Scan triggers on-demand secret decryption even if
    //    no network/ssh endpoint responds (returns structured error toast).
    let connect_btn = fresh_cx
        .debug_bounds("ssh-connect-0")
        .expect("ssh-connect-0");
    fresh_cx.simulate_click(connect_btn.center(), Modifiers::none());
    draw(fresh_cx);
    assert_eq!(counter2.load_count(), 1);
    let connected_or_errored = read(&fresh_view, fresh_cx, |app| {
        app.screen == Screen::Explore
            || app
                .notice
                .as_ref()
                .is_some_and(|(_, s)| *s == gpui_omarchy::Status::Error)
    });
    assert!(connected_or_errored);

    // 4b. Re-opening Remote while an SSH card is active performs 0 keychain
    //     reads and never triggers local mount discovery.
    let counter_session = CountingRootSecret::new([0x2A; 32]);
    update(&fresh_view, fresh_cx, |app, cx| {
        app.cred_store = Some(counter_session.store_for(&enc_path));
        app.active_ssh_session = None;
        app.active_ssh_profile_id = Some(updated[0].id.clone());
        app.remote = Some(updated[0].alias.clone());
        app.navigate_rail(Screen::Remote, cx);
    });
    draw(fresh_cx);
    assert_eq!(counter_session.load_count(), 0);
    assert!(read(&fresh_view, fresh_cx, |app| {
        app.active_ssh_session.is_none() && !app.volumes_loading
    }));

    // 5. Delete SSH card deletes credential first, then SQLite row.
    fresh_cx.simulate_resize(gpui_kit::size(px(1100.), px(900.)));
    draw(fresh_cx);
    let delete_btn =
        fresh_cx.debug_bounds("ssh-delete-0").expect("ssh-delete-0");
    fresh_cx.simulate_click(delete_btn.center(), Modifiers::none());
    draw(fresh_cx);
    assert!(
        StorageDb::open(&db_path)
            .expect("open db")
            .list_ssh_profiles()
            .expect("list profiles")
            .is_empty()
    );
    assert_eq!(
        counter2.store_for(&enc_path).get_ssh_secret(&updated[0].id),
        Ok(None)
    );
    assert!(fresh_cx.debug_bounds("ssh-card-0").is_none());

    // 6. Deleting a profile when device-credentials.enc does NOT exist never
    //    touches keychain and deletes SQLite cleanly (headless Linux safe).
    let missing_enc = cfg_dir.path().join("missing-credentials.enc");
    let counter3 = CountingRootSecret::new([0x33; 32]);
    StorageDb::open(&db_path)
        .expect("open db")
        .add_ssh_profile(&SshProfile {
            id: "no-enc-prof".into(),
            alias: "headless".into(),
            host: "10.0.0.9".into(),
            port: 22,
            username: "ops".into(),
            auth_mode: AuthMode::PrivateKey,
            default_path: None,
            created_at_secs: 1,
            updated_at_secs: 1,
            last_total_bytes: None,
            last_available_bytes: None,
            last_scanned_secs: None,
            pending_script_path: None,
            pending_script_items: None,
            pending_reclaim_bytes: None,
            pending_created_secs: None,
        })
        .expect("add profile");
    update(&fresh_view, fresh_cx, |app, cx| {
        app.cred_store = Some(counter3.store_for(&missing_enc));
        app.reload_ssh_profiles();
        app.delete_ssh_profile_card("no-enc-prof", cx);
    });
    assert_eq!(counter3.load_count(), 0);
    assert!(
        StorageDb::open(&db_path)
            .expect("open db")
            .list_ssh_profiles()
            .expect("list")
            .is_empty()
    );
}

#[gpui_kit::test]
fn remote_connect_confirm_modal_approve_deny_queue_and_fail_closed(
    cx: &mut TestAppContext,
) {
    use ruum_core::confirm::{
        RemoteConnectConfirmDecision, RemoteConnectConfirmRequest,
    };

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1000.), px(700.)));

    let make_req = |alias: &str| RemoteConnectConfirmRequest {
        target_label: alias.into(),
        alias: Some(alias.into()),
        host: "10.0.0.10".into(),
        port: 22,
        username: "ops".into(),
        auth_mode: "private_key".into(),
        tool_name: "scan_directory".into(),
    };

    let (tx1, rx1) = std::sync::mpsc::channel();
    let (tx2, rx2) = std::sync::mpsc::channel();
    let (tx3, rx3) = std::sync::mpsc::channel();

    update(&view, cx, |app, cx| {
        app.enqueue_remote_connect_confirm(
            make_req("prod-1"),
            Some(tx1),
            None,
            cx,
        );
        app.enqueue_remote_connect_confirm(
            make_req("prod-2"),
            Some(tx2),
            None,
            cx,
        );
        app.enqueue_remote_connect_confirm(
            make_req("prod-3"),
            Some(tx3),
            None,
            cx,
        );
    });
    draw(cx);

    assert!(cx.debug_bounds("remote-connect-confirm-popup").is_some());
    assert!(cx.debug_bounds("remote-connect-approve").is_some());
    assert!(cx.debug_bounds("remote-connect-deny").is_some());

    // Enter approves first request and promotes second request from queue.
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(rx1.try_recv(), Ok(RemoteConnectConfirmDecision::Approved));
    assert_eq!(
        read(&view, cx, |a| {
            a.remote_connect_confirm
                .as_ref()
                .map(|m| m.request.target_label.clone())
        }),
        Some("prod-2".into())
    );

    // Escape denies second request and promotes third.
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert_eq!(rx2.try_recv(), Ok(RemoteConnectConfirmDecision::Denied));
    assert_eq!(
        read(&view, cx, |a| {
            a.remote_connect_confirm
                .as_ref()
                .map(|m| m.request.target_label.clone())
        }),
        Some("prod-3".into())
    );

    // Dropping/clearing undecided modal fails closed with Denied.
    update(&view, cx, |app, _| {
        app.remote_connect_confirm = None;
    });
    assert_eq!(rx3.try_recv(), Ok(RemoteConnectConfirmDecision::Denied));

    // Standalone window writes Approved on click, and Denied on close.
    let resp_dir = tempfile::tempdir().expect("resp dir");
    let resp_approve = resp_dir.path().join("approve.json");
    let resp_close = resp_dir.path().join("close.json");
    let req_standalone = make_req("standalone-box");
    cx.update(|_, cx| {
        crate::open_remote_connect_confirm_window_with_mode(
            req_standalone.clone(),
            None,
            Some(resp_approve.clone()),
            false,
            cx,
        )
        .expect("open standalone approve window");
    });
    let windows = cx.windows();
    assert_eq!(windows.len(), 2);
    let mut win_cx = VisualTestContext::from_window(windows[1], &cx.cx);
    draw(&mut win_cx);
    let approve_btn = win_cx
        .debug_bounds("remote-connect-approve")
        .expect("approve btn");
    win_cx.simulate_click(approve_btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    let approved_json =
        std::fs::read_to_string(&resp_approve).expect("approve resp file");
    let parsed_approve: RemoteConnectConfirmDecision =
        serde_json::from_str(&approved_json).expect("parse approved");
    assert_eq!(parsed_approve, RemoteConnectConfirmDecision::Approved);

    // Second standalone window closed without clicking fails closed -> Denied.
    cx.update(|_, cx| {
        crate::open_remote_connect_confirm_window_with_mode(
            req_standalone,
            None,
            Some(resp_close.clone()),
            false,
            cx,
        )
        .expect("open standalone close window");
    });
    let windows = cx.windows();
    assert_eq!(windows.len(), 2);
    let mut close_cx = VisualTestContext::from_window(windows[1], &cx.cx);
    draw(&mut close_cx);
    close_cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let closed_json =
        std::fs::read_to_string(&resp_close).expect("close resp file");
    let parsed_close: RemoteConnectConfirmDecision =
        serde_json::from_str(&closed_json).expect("parse denied");
    assert_eq!(parsed_close, RemoteConnectConfirmDecision::Denied);
}

#[gpui_kit::test]
fn settings_general_watchdog_webhook_and_ai_round_trip_in_sqlite(
    cx: &mut TestAppContext,
) {
    use ruum_core::storage::{AiConfig, AppSettings, StorageDb};
    use ruum_core::watchdog_config::AiCliProvider;
    use ruum_watchdog::WatchdogConfig;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");

    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1100.), px(1400.)));
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(cx);

    assert!(cx.debug_bounds("setting-general-section").is_some());
    assert!(cx.debug_bounds("setting-tab-general").is_some());
    assert!(cx.debug_bounds("setting-tab-watchdog").is_some());
    assert!(cx.debug_bounds("setting-tab-webhook").is_some());
    assert!(cx.debug_bounds("setting-tab-ai").is_some());
    assert!(cx.debug_bounds("setting-tab-cli").is_some());

    // Switch to Webhook category and verify multi-webhook validation + CRUD.
    let wh_tab = cx.debug_bounds("setting-tab-webhook").expect("webhook tab");
    cx.simulate_click(wh_tab.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(cx.debug_bounds("setting-webhook-section").is_some());
    update(&view, cx, Ruum::open_add_webhook_form);
    draw(cx);
    assert!(cx.debug_bounds("setting-webhook-form").is_some());

    // Invalid webhook URL & invalid headers JSON abort Save cleanly.
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.webhook_input_name
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("ops", window, cx));
            app.setting_input_webhook_url
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("ftp://invalid", window, cx));
            app.save_webhook_form(cx);
        });
    });
    assert!(read(&view, cx, |a| a.webhook_form_error.is_some()));
    assert!(read(&view, cx, |a| a.webhooks.is_empty()));

    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.setting_input_webhook_url.as_ref().unwrap().update(
                cx,
                |i, cx| {
                    i.set_value("https://hooks.example.com/ruum", window, cx);
                },
            );
            app.setting_input_webhook_headers
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("[1,2]", window, cx));
            app.save_webhook_form(cx);
        });
    });
    assert!(read(&view, cx, |a| a.webhook_form_error.is_some()));

    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.webhook_form_enabled = true;
            app.setting_input_webhook_timeout
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("15", window, cx));
            app.setting_input_webhook_headers.as_ref().unwrap().update(
                cx,
                |i, cx| {
                    i.set_value("{\"X-Token\":\"abc\"}", window, cx);
                },
            );
            app.save_webhook_form(cx);
        });
    });
    assert_eq!(read(&view, cx, |a| a.webhooks.len()), 1);

    // Save valid settings across General + Watchdog + AI without clobbering
    // saved multi-webhook endpoints.
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.set_settings_category(
                crate::state::SettingsCategory::General,
                cx,
            );
            app.watchdog_draft.power_preset =
                crate::power::PowerEfficiency::Miser;
            app.watchdog_draft.apparent_size = true;
            app.watchdog_draft.include_hidden = true;
            app.watchdog_draft.follow_system_appearance = false;
            app.setting_input_default_depth
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("5", window, cx));
            app.setting_input_ssh_duration
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("30", window, cx));

            app.set_settings_category(
                crate::state::SettingsCategory::Watchdog,
                cx,
            );
            app.watchdog_draft.enabled = true;
            app.watchdog_input_min_free
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("35", window, cx));

            app.set_settings_category(crate::state::SettingsCategory::Ai, cx);
            app.watchdog_draft.ai_auto_analyze = false;
            app.set_watchdog_provider(AiCliProvider::Pi, cx);
            app.watchdog_input_model
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| {
                    i.set_value("claude-sonnet-4-6", window, cx);
                });

            app.save_watchdog_settings(window, cx);
        });
    });
    assert_eq!(
        read(&view, cx, |a| a.notice.as_ref().map(|(_, s)| *s)),
        Some(gpui_omarchy::Status::Success)
    );

    let db = StorageDb::open(&db_path).expect("reopen db");
    let saved_app = db.load_app_settings().expect("app settings");
    assert!(saved_app.updated_at_secs > 0);
    assert_eq!(
        saved_app,
        AppSettings {
            power_preset: "miser".into(),
            default_depth: 5,
            apparent_size: true,
            include_hidden: true,
            follow_system_appearance: false,
            ssh_session_duration_minutes: 30,
            updated_at_secs: saved_app.updated_at_secs,
            ..AppSettings::default()
        }
    );
    let saved_wd = db.watchdog().expect("wd").load_config().expect("wd cfg");
    assert!(saved_wd.updated_at_secs > 0);
    assert_eq!(
        saved_wd,
        WatchdogConfig {
            enabled: true,
            min_free_gb: 35,
            updated_at_secs: saved_wd.updated_at_secs,
            ..WatchdogConfig::default()
        }
    );
    let saved_webhooks = db.list_webhooks().expect("webhooks");
    assert_eq!(saved_webhooks.len(), 1);
    assert_eq!(saved_webhooks[0].name, "ops");
    assert_eq!(saved_webhooks[0].url, "https://hooks.example.com/ruum");
    assert_eq!(saved_webhooks[0].timeout_secs, 15);
    assert_eq!(saved_webhooks[0].headers_json, "{\"X-Token\":\"abc\"}");
    assert!(saved_webhooks[0].enabled);

    let saved_ai = db.load_ai_config().expect("ai cfg");
    assert!(saved_ai.updated_at_secs > 0);
    assert_eq!(
        saved_ai,
        AiConfig {
            provider: AiCliProvider::Pi,
            custom_command: String::new(),
            extra_args: String::new(),
            model: Some("claude-sonnet-4-6".into()),
            auto_analyze_on_alert: false,
            updated_at_secs: saved_ai.updated_at_secs,
            ..AiConfig::default()
        }
    );

    // Fresh Ruum loads identical state back from SQLite ruum.db.
    let (fresh_view, fresh_cx) = view_over(temp.path(), &mut cx.cx);
    update(&fresh_view, fresh_cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.reload_watchdog_config_and_prepare_settings();
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(fresh_cx);
    read(&fresh_view, fresh_cx, |app| {
        assert_eq!(app.app_settings.default_depth, 5);
        assert!(app.app_settings.apparent_size);
        assert!(app.app_settings.include_hidden);
        assert!(!app.app_settings.follow_system_appearance);
        assert_eq!(app.app_settings.ssh_session_duration_minutes, 30);
        assert_eq!(app.webhooks, saved_webhooks);
        assert_eq!(app.ai_config, saved_ai);
        assert_eq!(app.watchdog_config, saved_wd);
    });
}

#[gpui_kit::test]
fn host_tabs_strip_preserves_sessions_and_local_tab_is_unclosable(
    cx: &mut TestAppContext,
) {
    use crate::state::{ExploreHostTab, ExploreSessionSnapshot, HostTabKey};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);

    // Mark a path on the local session.
    update(&view, cx, |app, cx| {
        let crumbs = app.crumbs_for_path(&temp.path().join("junk")).unwrap();
        app.select(Some(crumbs), cx);
        app.toggle_mark_selected(cx);
    });
    let local_root = read(&view, cx, |a| a.root_path.clone());
    let local_marks_len = read(&view, cx, |a| a.marks.len());
    assert_eq!(local_marks_len, 1);

    draw(cx);
    assert!(cx.debug_bounds("host-tabs").is_some());
    assert!(cx.debug_bounds("host-tab-local").is_some());
    assert!(cx.debug_bounds("host-tab-close-local").is_none());

    // Add a remote SSH tab with its own snapshot tree and switch to it.
    let remote_fixture = fixture();
    let remote_tree =
        std::sync::Arc::new(scan(remote_fixture.path(), options()).unwrap());
    let mut remote_marks = crate::marks::Marks::default();
    remote_marks.toggle(ruum_core::removal::Target {
        path: PathBuf::from("/srv/remote/var.log"),
        bytes: 2048,
        is_dir: false,
        hidden: false,
    });
    let mut remote_snap = ExploreSessionSnapshot::default();
    remote_snap.root_path = PathBuf::from("/srv/remote");
    remote_snap.scan_root = PathBuf::from("/srv/remote");
    remote_snap.tree = Some(std::sync::Arc::clone(&remote_tree));
    remote_snap.scanned_at = 100;
    remote_snap.marks = remote_marks;
    remote_snap.remote = Some("prod-1".into());
    remote_snap.remote_platform = "linux".into();
    remote_snap.active_ssh_profile_id = Some("srv-1".into());
    update(&view, cx, |app, cx| {
        app.host_tabs.push(ExploreHostTab {
            key: HostTabKey::Ssh("srv-1".into()),
            label: "prod-1".into(),
            session: Some(remote_snap),
        });
        cx.notify();
    });
    draw(cx);
    let ssh_tab = cx.debug_bounds("host-tab-0").expect("ssh tab 0");
    assert!(cx.debug_bounds("host-tab-close-0").is_some());
    cx.simulate_click(ssh_tab.center(), gpui_kit::Modifiers::none());
    draw(cx);

    read(&view, cx, |app| {
        assert_eq!(app.active_host_tab, HostTabKey::Ssh("srv-1".into()));
        assert_eq!(app.remote.as_deref(), Some("prod-1"));
        assert_eq!(app.root_path, PathBuf::from("/srv/remote"));
        assert_eq!(app.marks.len(), 1);
        assert!(app.tree.is_some());
    });

    // Click Local tab -> restores local tree & marks intact!
    let local_tab = cx.debug_bounds("host-tab-local").expect("local tab");
    cx.simulate_click(local_tab.center(), gpui_kit::Modifiers::none());
    draw(cx);
    read(&view, cx, |app| {
        assert_eq!(app.active_host_tab, HostTabKey::Local);
        assert!(app.remote.is_none());
        assert_eq!(app.root_path, local_root);
        assert_eq!(app.marks.len(), local_marks_len);
        assert!(app.tree.is_some());
    });

    // Switch back to SSH tab and click its close button -> falls back to Local.
    let ssh_tab = cx.debug_bounds("host-tab-0").expect("ssh tab 0");
    cx.simulate_click(ssh_tab.center(), gpui_kit::Modifiers::none());
    draw(cx);
    let close_btn = cx.debug_bounds("host-tab-close-0").expect("ssh close btn");
    cx.simulate_click(close_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    read(&view, cx, |app| {
        assert_eq!(app.active_host_tab, HostTabKey::Local);
        assert_eq!(app.host_tabs.len(), 1);
        assert!(app.remote.is_none());
        assert_eq!(app.root_path, local_root);
        assert!(app.tree.is_some());
    });

    // Local tab can never be closed.
    update(&view, cx, |app, cx| {
        app.close_host_tab(&HostTabKey::Local, cx);
    });
    assert_eq!(read(&view, cx, |a| a.host_tabs.len()), 1);
}

#[gpui_kit::test]
fn remote_ssh_tilde_default_and_action_icon_buttons_and_spinners(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");

    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.navigate_rail(Screen::Remote, cx);
    });
    cx.run_until_parked();
    draw(cx);

    let add_btn = cx.debug_bounds("ssh-add-btn").expect("ssh add icon button");
    cx.simulate_click(add_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    let val = cx.update(|_, cx| {
        view.read(cx)
            .ssh_input_default_path
            .as_ref()
            .unwrap()
            .read(cx)
            .value()
            .to_string()
    });
    assert_eq!(val, "~");

    // Save SSH profile directly to db and verify card icon buttons + spinners.
    {
        let db = ruum_core::storage::StorageDb::open(&db_path).expect("db");
        db.add_ssh_profile(&ruum_core::storage::SshProfile {
            id: String::new(),
            alias: "staging".into(),
            host: "10.0.0.5".into(),
            port: 22,
            username: "deploy".into(),
            auth_mode: ruum_core::storage::AuthMode::PrivateKey,
            default_path: Some("~".into()),
            created_at_secs: 1,
            updated_at_secs: 1,
            last_total_bytes: None,
            last_available_bytes: None,
            last_scanned_secs: None,
            pending_script_path: None,
            pending_script_items: None,
            pending_reclaim_bytes: None,
            pending_created_secs: None,
        })
        .expect("add profile");
    }
    update(&view, cx, |app, cx| {
        app.close_ssh_form(cx);
        app.reload_ssh_profiles();
    });
    draw(cx);
    assert!(cx.debug_bounds("ssh-connect-0").is_some());
    assert!(cx.debug_bounds("ssh-browse-0").is_some());
    assert!(cx.debug_bounds("ssh-check-0").is_some());
    assert!(cx.debug_bounds("ssh-ai-0").is_some());
    assert!(cx.debug_bounds("ssh-edit-0").is_some());
    assert!(cx.debug_bounds("ssh-delete-0").is_some());

    // Spinners render when checking/analyzing profile.
    let id = read(&view, cx, |a| a.ssh_profiles[0].id.clone());
    update(&view, cx, |app, cx| {
        app.ssh_checking_profiles.insert(id.clone());
        app.ssh_analyzing_profiles.insert(id);
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("ssh-check-spin-0").is_some());
    assert!(cx.debug_bounds("ssh-ai-spin-0").is_some());
}

#[gpui_kit::test]
fn local_cleanup_prunes_ai_proposal_and_remote_modal_never_touches_local(
    cx: &mut TestAppContext,
) {
    use ruum_core::removal::RemovalMode;
    use ruum_core::watchdog_config::{CleanupProposal, ProposalItem};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");

    let file_a = temp.path().join("a.log");
    let file_b = temp.path().join("b.log");
    std::fs::write(&file_a, b"aaaa").expect("write a");
    std::fs::write(&file_b, b"bbbb").expect("write b");

    let proposal = CleanupProposal {
        timestamp_secs: 100,
        summary: "Two stale logs".into(),
        items: vec![
            ProposalItem::new(
                file_a.clone(),
                4,
                RemovalMode::Permanent,
                Some("stale".into()),
                None,
            ),
            ProposalItem::new(
                file_b.clone(),
                4,
                RemovalMode::Permanent,
                Some("stale".into()),
                None,
            ),
        ],
        docker_targets: Vec::new(),
    };
    {
        let db = ruum_core::storage::StorageDb::open(&db_path).expect("db");
        let ai_state =
            ruum_core::storage::AiState::from_proposal(&proposal, 100)
                .expect("ai state");
        db.save_ai_state(&ai_state).expect("save ai state");
    }

    let (view, cx) = view_over(temp.path(), cx);
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.reload_watchdog_state();
        app.navigate_rail(Screen::Watchdog, cx);
    });
    assert_eq!(
        read(&view, cx, |a| a.ai_proposal.as_ref().map(|p| p.items.len())),
        Some(2)
    );

    // Confirm cleaning only `file_a` -> `file_b` remains in Watchdog proposal.
    let req = crate::state::proposal_confirm_request(
        temp.path().to_path_buf(),
        &proposal,
        crate::i18n::Locale::En,
    );
    let win = cx
        .update(|_, cx| {
            crate::open_local_confirm_window(
                req,
                Some(db_path.clone()),
                ruum_core::docker_adapter::DockerProbeStatus::NotInstalled,
                cx,
            )
        })
        .expect("confirm win");
    let mut confirm_cx = VisualTestContext::from_window(win.into(), &cx.cx);
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

    assert!(!file_a.exists());
    assert!(file_b.exists());
    let remaining = read(&view, cx, |a| {
        a.ai_proposal
            .as_ref()
            .map(|p| p.items.iter().map(|i| i.path.clone()).collect::<Vec<_>>())
    });
    assert_eq!(remaining, Some(vec![file_b.clone()]));

    // Confirm remaining `file_b` -> proposal becomes None.
    let prop_b = read(&view, cx, |a| a.ai_proposal.clone().unwrap());
    let req_b = crate::state::proposal_confirm_request(
        temp.path().to_path_buf(),
        &prop_b,
        crate::i18n::Locale::En,
    );
    let win_b = cx
        .update(|_, cx| {
            crate::open_local_confirm_window(
                req_b,
                Some(db_path),
                ruum_core::docker_adapter::DockerProbeStatus::NotInstalled,
                cx,
            )
        })
        .expect("confirm win b");
    let mut confirm_cx_b = VisualTestContext::from_window(win_b.into(), &cx.cx);
    draw(&mut confirm_cx_b);
    let confirm_btn_b = confirm_cx_b
        .debug_bounds("confirm-clean")
        .expect("confirm button b");
    confirm_cx_b
        .simulate_click(confirm_btn_b.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();

    assert!(!file_b.exists());
    assert!(read(&view, cx, |a| a.ai_proposal.is_none()));

    // Remote confirmation modal carries `remote_profile_id` and stays isolated.
    let remote_modal = crate::state::CleanupConfirmModal::new(
        crate::state::remote_proposal_confirm_request(
            PathBuf::from("/srv/home"),
            "prod",
            &proposal,
            crate::i18n::Locale::En,
        ),
        None,
        None,
    )
    .with_remote_profile("srv-1".into());
    assert_eq!(remote_modal.remote_profile_id.as_deref(), Some("srv-1"));
    assert!(read(&view, cx, |a| a.ai_proposal.is_none()));
}

#[gpui_kit::test]
fn settings_categories_preserve_drafts_webhook_crud_guide_and_icon_model_select(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");

    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1100.), px(900.)));
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(cx);

    // Edit General draft & switch across categories -> draft is preserved!
    update(&view, cx, |app, cx| {
        app.watchdog_draft.apparent_size = true;
        app.set_settings_category(crate::state::SettingsCategory::Webhook, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("setting-webhook-guide-toggle").is_some());
    assert!(cx.debug_bounds("setting-webhook-guide").is_some());

    let toggle_guide = cx
        .debug_bounds("setting-webhook-guide-toggle")
        .expect("guide toggle");
    cx.simulate_click(toggle_guide.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(cx.debug_bounds("setting-webhook-guide").is_none());

    let toggle_guide = cx
        .debug_bounds("setting-webhook-guide-toggle")
        .expect("guide toggle");
    cx.simulate_click(toggle_guide.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(cx.debug_bounds("setting-webhook-guide").is_some());

    // Add -> Edit -> Toggle -> Delete webhook in UI.
    let add_wh = cx
        .debug_bounds("setting-webhook-add-btn")
        .expect("add webhook btn");
    cx.simulate_click(add_wh.center(), gpui_kit::Modifiers::none());
    draw(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.webhook_input_name
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("alerts", window, cx));
            app.setting_input_webhook_url.as_ref().unwrap().update(
                cx,
                |i, cx| {
                    i.set_value("https://hooks.example.com/a", window, cx);
                },
            );
            app.save_webhook_form(cx);
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("setting-webhook-row-0").is_some());
    assert!(cx.debug_bounds("setting-webhook-test-0").is_some());
    assert!(cx.debug_bounds("setting-webhook-edit-0").is_some());
    assert!(cx.debug_bounds("setting-webhook-delete-0").is_some());

    let toggle_wh = cx
        .debug_bounds("setting-webhook-toggle-0")
        .expect("webhook toggle");
    cx.simulate_click(toggle_wh.center(), gpui_kit::Modifiers::none());
    assert!(!read(&view, cx, |a| a.webhooks[0].enabled));

    let del_wh = cx
        .debug_bounds("setting-webhook-delete-0")
        .expect("webhook delete");
    cx.simulate_click(del_wh.center(), gpui_kit::Modifiers::none());
    assert!(read(&view, cx, |a| a.webhooks.is_empty()));

    // Switch back to General -> draft preserved, then check AI icon-only select.
    update(&view, cx, |app, cx| {
        app.set_settings_category(crate::state::SettingsCategory::General, cx);
    });
    assert!(read(&view, cx, |a| a.watchdog_draft.apparent_size));
    update(&view, cx, |app, cx| {
        app.watchdog_models = vec![ruum_core::ai_cli::DiscoveredModel {
            id: "m1".into(),
            label: "M1".into(),
            provider: None,
        }];
        app.set_settings_category(crate::state::SettingsCategory::Ai, cx);
    });
    draw(cx);
    let sel_btn = cx
        .debug_bounds("setting-model-select-0")
        .expect("icon-only model select");
    assert!(
        sel_btn.size.width < px(60.),
        "expected compact icon-only model select button, got {sel_btn:?}"
    );
}

#[gpui_kit::test]
fn local_confirm_cleanup_posts_started_then_done_notifications_and_toast(
    cx: &mut TestAppContext,
) {
    use ruum_core::confirm::{CleanupConfirmItem, CleanupConfirmRequest};
    use ruum_core::watchdog::{
        NOTIF_TAG_CLEANUP_DONE, NOTIF_TAG_CLEANUP_STARTED,
    };

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let target = temp.path().join("junk/stale.bin");
    std::fs::write(&target, vec![b'x'; 2048]).expect("write target");

    let tree = scan(temp.path(), options()).expect("scan fixture");
    let root_path = temp.path().to_path_buf();
    let db_for_win = db_path.clone();
    let main_win = cx.add_window(move |_, cx| {
        let mut app = Ruum::with_tree(root_path, tree, options(), 3, cx);
        app.locale = crate::i18n::Locale::Zh;
        app.db_path = Some(db_for_win);
        app
    });
    cx.update(|cx| {
        crate::install_notification_handler(cx);
        crate::install_test_runtime(
            temp.path().to_path_buf(),
            false,
            Some(main_win),
            cx,
        );
    });

    let req = CleanupConfirmRequest {
        root: temp.path().to_path_buf(),
        title: "Local AI Cleanup".into(),
        summary: Some("Confirm stale files".into()),
        items: vec![CleanupConfirmItem {
            path: target.clone(),
            bytes: 2048,
            mode: RemovalMode::Permanent,
            reason: Some("stale".into()),
            git_warning: None,
        }],
        docker_items: Vec::new(),
        docker_permission_hint: None,
    };
    let win = cx
        .update(|cx| {
            crate::open_local_confirm_window(
                req,
                Some(db_path),
                ruum_core::docker_adapter::DockerProbeStatus::NotInstalled,
                cx,
            )
        })
        .expect("open local confirm window");
    win.update(cx, |confirm_win, _, _| {
        confirm_win.set_locale(crate::i18n::Locale::Zh);
    })
    .expect("set locale");

    let mut confirm_cx = VisualTestContext::from_window(win.into(), cx);
    draw(&mut confirm_cx);
    let confirm_btn = confirm_cx
        .debug_bounds("confirm-clean")
        .expect("confirm button");
    confirm_cx
        .simulate_click(confirm_btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();

    assert!(!target.exists());
    let after = cx.shown_system_notifications();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].tag.as_ref(), NOTIF_TAG_CLEANUP_STARTED);
    assert_eq!(
        after[0].title.as_ref(),
        crate::i18n::messages::cleanup_started_notif_title(
            crate::i18n::Locale::Zh
        )
    );
    assert!(after[0].body.contains("正在后台清理"));
    assert!(after[0].body.contains("请稍等"));
    assert_eq!(after[1].tag.as_ref(), NOTIF_TAG_CLEANUP_DONE);
    assert_eq!(
        after[1].title.as_ref(),
        crate::i18n::messages::cleanup_complete_notif_title(
            crate::i18n::Locale::Zh
        )
    );
    assert_eq!(
        main_win
            .read_with(cx, |a, _| a.notice.as_ref().map(|(_, s)| *s))
            .expect("read notice"),
        Some(gpui_omarchy::Status::Success)
    );

    // Also verify synchronous immediate state before background tasks drain:
    // calling `confirm` directly emits `NOTIF_TAG_CLEANUP_STARTED` and sets
    // `Status::Neutral` on the open main window prior to `run_until_parked()`.
    let target2 = temp.path().join("junk/stale2.bin");
    std::fs::write(&target2, vec![b'y'; 1024]).expect("write target2");
    let req2 = CleanupConfirmRequest {
        root: temp.path().to_path_buf(),
        title: "Local AI Cleanup 2".into(),
        summary: None,
        items: vec![CleanupConfirmItem {
            path: target2.clone(),
            bytes: 1024,
            mode: RemovalMode::Permanent,
            reason: None,
            git_warning: None,
        }],
        docker_items: Vec::new(),
        docker_permission_hint: None,
    };
    let win2 = cx
        .update(|cx| {
            crate::open_local_confirm_window(
                req2,
                None,
                ruum_core::docker_adapter::DockerProbeStatus::NotInstalled,
                cx,
            )
        })
        .expect("open second local confirm window");
    win2.update(cx, |confirm_win, window, cx| {
        confirm_win.set_locale(crate::i18n::Locale::En);
        confirm_win.confirm(window, cx);
    })
    .expect("confirm second window");

    let immediate = cx.shown_system_notifications();
    assert_eq!(immediate.len(), 3);
    assert_eq!(immediate[2].tag.as_ref(), NOTIF_TAG_CLEANUP_STARTED);
    assert_eq!(
        main_win
            .read_with(cx, |a, _| a.notice.as_ref().map(|(_, s)| *s))
            .expect("read immediate neutral notice"),
        Some(gpui_omarchy::Status::Neutral)
    );

    cx.run_until_parked();
    assert!(!target2.exists());
    let final_notifs = cx.shown_system_notifications();
    assert_eq!(final_notifs.len(), 4);
    assert_eq!(final_notifs[3].tag.as_ref(), NOTIF_TAG_CLEANUP_DONE);
    assert_eq!(
        main_win
            .read_with(cx, |a, _| a.notice.as_ref().map(|(_, s)| *s))
            .expect("read final success notice"),
        Some(gpui_omarchy::Status::Success)
    );
}

#[gpui_kit::test]
fn ipc_and_standalone_confirm_notify_only_on_nonempty_confirm(
    cx: &mut TestAppContext,
) {
    use ruum_core::confirm::{
        CleanupConfirmDecision, CleanupConfirmItem, CleanupConfirmRequest,
    };
    use ruum_core::watchdog::NOTIF_TAG_CLEANUP_STARTED;
    use std::sync::mpsc::channel;

    cx.update(gpui_omarchy::init);
    cx.update(|cx| crate::install_notification_handler(cx));
    let temp = fixture();
    let item_path = temp.path().join("junk/blob.bin");
    let req = CleanupConfirmRequest {
        root: temp.path().to_path_buf(),
        title: "External Cleanup".into(),
        summary: None,
        items: vec![CleanupConfirmItem {
            path: item_path,
            bytes: 4096,
            mode: RemovalMode::Trash,
            reason: None,
            git_warning: None,
        }],
        docker_items: Vec::new(),
        docker_permission_hint: None,
    };

    // 1. Cancel does NOT emit any notification.
    let (cancel_tx, cancel_rx) = channel();
    let cancel_win = cx
        .update(|cx| {
            crate::open_confirm_window(req.clone(), Some(cancel_tx), None, cx)
        })
        .expect("open cancel win");
    let mut cancel_cx = VisualTestContext::from_window(cancel_win.into(), cx);
    draw(&mut cancel_cx);
    let cancel_btn = cancel_cx
        .debug_bounds("confirm-cancel")
        .expect("cancel button");
    cancel_cx.simulate_click(cancel_btn.center(), gpui_kit::Modifiers::none());
    assert_eq!(
        cancel_rx.recv().expect("decision"),
        CleanupConfirmDecision::Cancelled
    );
    assert!(cx.shown_system_notifications().is_empty());

    // 2. Confirm with zero selected items does NOT emit any notification.
    let (zero_tx, _zero_rx) = channel();
    let zero_win = cx
        .update(|cx| {
            crate::open_confirm_window(req.clone(), Some(zero_tx), None, cx)
        })
        .expect("open zero win");
    let mut zero_cx = VisualTestContext::from_window(zero_win.into(), cx);
    draw(&mut zero_cx);
    let check_0 = zero_cx.debug_bounds("check-item-0").expect("checkbox 0");
    zero_cx.simulate_click(check_0.center(), gpui_kit::Modifiers::none());
    draw(&mut zero_cx);
    let confirm_zero = zero_cx
        .debug_bounds("confirm-clean")
        .expect("confirm button");
    zero_cx.simulate_click(confirm_zero.center(), gpui_kit::Modifiers::none());
    assert!(cx.shown_system_notifications().is_empty());
    zero_win
        .update(cx, crate::CleanupConfirmWindow::cancel)
        .expect("close zero win");

    // 3. IPC confirm with selection emits `NOTIF_TAG_CLEANUP_STARTED`.
    let (ipc_tx, ipc_rx) = channel();
    let ipc_win = cx
        .update(|cx| {
            crate::open_confirm_window(req.clone(), Some(ipc_tx), None, cx)
        })
        .expect("open ipc win");
    let mut ipc_cx = VisualTestContext::from_window(ipc_win.into(), cx);
    draw(&mut ipc_cx);
    let confirm_ipc = ipc_cx
        .debug_bounds("confirm-clean")
        .expect("confirm button");
    ipc_cx.simulate_click(confirm_ipc.center(), gpui_kit::Modifiers::none());
    assert!(matches!(
        ipc_rx.recv().expect("ipc decision"),
        CleanupConfirmDecision::Confirmed { selected_paths, .. }
            if selected_paths.len() == 1
    ));
    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].tag.as_ref(), NOTIF_TAG_CLEANUP_STARTED);

    // 4. Standalone `--confirm-cleanup` mode emits `NOTIF_TAG_CLEANUP_STARTED`.
    let resp_dir = tempfile::tempdir().expect("resp dir");
    let resp_file = resp_dir.path().join("resp.json");
    let (standalone_win, standalone_cx) = cx.add_window_view(|_, cx| {
        crate::CleanupConfirmWindow::new(
            req,
            None,
            Some(resp_file.clone()),
            true,
            cx,
        )
    });
    draw(standalone_cx);
    let confirm_standalone = standalone_cx
        .debug_bounds("confirm-clean")
        .expect("standalone confirm");
    standalone_cx.simulate_click(
        confirm_standalone.center(),
        gpui_kit::Modifiers::none(),
    );
    assert!(resp_file.exists());
    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[1].tag.as_ref(), NOTIF_TAG_CLEANUP_STARTED);
    let _ = standalone_win;
}

#[gpui_kit::test]
fn docker_probe_status_persists_across_host_tabs_and_renders_page_states(
    cx: &mut TestAppContext,
) {
    use crate::state::{ExploreHostTab, ExploreSessionSnapshot, HostTabKey};
    use ruum_core::docker_adapter::{
        DockerImageEntry, DockerProbeStatus, DockerUsageSummary,
    };

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);

    // 1. Explore never displays inline Docker banner; when local Docker is
    //    NotInstalled/DaemonStopped/PermissionDenied, `rail-docker` stays
    //    hidden, `navigate_rail(Screen::Docker)` is rejected, and
    //    `remote-docker-btn` is hidden on the Local tab.
    draw(cx);
    assert!(cx.debug_bounds("docker-status-line").is_none());
    assert!(cx.debug_bounds("rail-docker").is_none());
    assert!(cx.debug_bounds("remote-docker-btn").is_none());
    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Docker, cx);
    });
    draw(cx);
    assert_eq!(read(&view, cx, |a| a.screen), Screen::Explore);

    update(&view, cx, |app, cx| {
        app.apply_local_docker_status(DockerProbeStatus::PermissionDenied {
            socket_hint: "/var/run/docker.sock".into(),
        });
        app.navigate_rail(Screen::Docker, cx);
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("rail-docker").is_none());
    assert_eq!(read(&view, cx, |a| a.screen), Screen::Explore);

    // 2. Available on Local reveals `rail-docker` and renders local overview &
    //    cards in `Screen::Docker`, while Explore stays decoupled.
    let local_docker = DockerProbeStatus::Available(DockerUsageSummary {
        build_cache_reclaimable_bytes: 1024 * 1024,
        build_cache_shared_bytes: 512 * 1024,
        removable_images: vec![DockerImageEntry {
            id: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            repository: "<none>".into(),
            tag: "<none>".into(),
            created_since: "2d".into(),
            size_bytes: 2 * 1024 * 1024,
            unique_bytes: 2 * 1024 * 1024,
            dangling: true,
        }],
        detailed: true,
        ..DockerUsageSummary::default()
    });
    update(&view, cx, |app, cx| {
        app.apply_local_docker_status(local_docker.clone());
        app.navigate_rail(Screen::Docker, cx);
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("rail-docker").is_some());
    assert!(cx.debug_bounds("docker-overview").is_some());
    assert!(cx.debug_bounds("docker-card-images").is_some());
    assert!(cx.debug_bounds("docker-card-containers").is_some());
    assert!(cx.debug_bounds("docker-card-build-cache").is_some());
    assert!(cx.debug_bounds("docker-build-cache-shared").is_some());
    assert!(cx.debug_bounds("docker-card-volumes").is_some());
    assert!(cx.debug_bounds("docker-volumes-protected").is_some());

    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Explore, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("docker-status-line").is_none());
    assert!(cx.debug_bounds("remote-docker-btn").is_none());

    // 3. Switching to a remote tab with PermissionDenied hides
    //    `remote-docker-btn`, while `Screen::Docker` continues showing LOCAL
    //    Docker cleanly (decoupled from active remote tab).
    let mut denied_snap = ExploreSessionSnapshot::default();
    denied_snap.root_path = PathBuf::from("/srv/app");
    denied_snap.scan_root = PathBuf::from("/srv/app");
    denied_snap.tree =
        Some(std::sync::Arc::new(scan(temp.path(), options()).unwrap()));
    denied_snap.remote = Some("prod-denied".into());
    denied_snap.remote_platform = "linux".into();
    denied_snap.docker = DockerProbeStatus::PermissionDenied {
        socket_hint: "/var/run/docker.sock".into(),
    };
    update(&view, cx, |app, cx| {
        app.host_tabs.push(ExploreHostTab {
            key: HostTabKey::Ssh("srv-denied".into()),
            label: "prod-denied".into(),
            session: Some(denied_snap),
        });
        app.switch_host_tab(HostTabKey::Ssh("srv-denied".into()), cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("remote-docker-btn").is_none());
    assert!(cx.debug_bounds("rail-docker").is_some());
    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Docker, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("docker-overview").is_some());
    assert!(cx.debug_bounds("docker-permission-denied").is_none());

    // 4. Switching to a remote tab with Available Docker renders
    //    `remote-docker-btn` beside `trail-switch-root`, opens
    //    `remote-docker-modal` on click, closes on Escape, and auto-closes on
    //    host tab switch.
    let mut avail_snap = ExploreSessionSnapshot::default();
    avail_snap.root_path = PathBuf::from("/srv/docker");
    avail_snap.scan_root = PathBuf::from("/srv/docker");
    avail_snap.tree =
        Some(std::sync::Arc::new(scan(temp.path(), options()).unwrap()));
    avail_snap.remote = Some("prod-docker".into());
    avail_snap.remote_platform = "linux".into();
    avail_snap.docker = DockerProbeStatus::Available(DockerUsageSummary {
        build_cache_reclaimable_bytes: 500_000_000,
        detailed: true,
        ..DockerUsageSummary::default()
    });
    update(&view, cx, |app, cx| {
        app.host_tabs.push(ExploreHostTab {
            key: HostTabKey::Ssh("srv-docker".into()),
            label: "prod-docker".into(),
            session: Some(avail_snap),
        });
        app.switch_host_tab(HostTabKey::Ssh("srv-docker".into()), cx);
    });
    draw(cx);
    let switch_root_bounds = cx
        .debug_bounds("trail-switch-root")
        .expect("trail-switch-root");
    let remote_docker_btn = cx
        .debug_bounds("remote-docker-btn")
        .expect("remote-docker-btn");
    assert!(remote_docker_btn.origin.x > switch_root_bounds.origin.x);

    cx.simulate_click(remote_docker_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(cx.debug_bounds("remote-docker-modal").is_some());
    assert!(cx.debug_bounds("docker-overview").is_some());
    assert!(cx.debug_bounds("docker-build-cache").is_some());
    assert!(cx.debug_bounds("docker-generate-script").is_some());

    // Escape closes remote Docker modal without leaving Explore.
    press(cx, "escape");
    draw(cx);
    assert!(cx.debug_bounds("remote-docker-modal").is_none());
    assert_eq!(read(&view, cx, |a| a.screen), Screen::Explore);

    // Re-open remote Docker modal then switch back to Local -> auto-closes.
    cx.simulate_click(remote_docker_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(read(&view, cx, |a| a.remote_docker_modal_open));
    update(&view, cx, |app, cx| {
        app.switch_host_tab(HostTabKey::Local, cx);
    });
    draw(cx);
    assert!(!read(&view, cx, |a| a.remote_docker_modal_open));
    assert!(cx.debug_bounds("remote-docker-btn").is_none());

    // 5. Updating local docker to DaemonStopped while on Screen::Docker returns
    //    to Explore and hides `rail-docker`.
    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Docker, cx);
    });
    draw(cx);
    assert_eq!(read(&view, cx, |a| a.screen), Screen::Docker);
    update(&view, cx, |app, cx| {
        app.apply_local_docker_status(DockerProbeStatus::DaemonStopped);
        cx.notify();
    });
    draw(cx);
    assert_eq!(read(&view, cx, |a| a.screen), Screen::Explore);
    assert!(cx.debug_bounds("rail-docker").is_none());
}

#[gpui_kit::test]
fn slice3_remote_snapshot_ai_analysis_and_platform_script_selection(
    cx: &mut TestAppContext,
) {
    use ruum_core::ai_cli::{CommandOutput, CommandRunner};
    use ruum_core::docker_adapter::{
        DockerCleanTarget, DockerProbeStatus, DockerUsageSummary,
    };
    use ruum_core::identity::HostIdentity;
    use ruum_core::remote::{Header, Snapshot};
    use ruum_core::storage::{AiConfig, AuthMode, SshProfile};
    use ruum_core::tree::{Metric, Node, NodeKind, aggregate};
    use ruum_core::watchdog_config::AiCliProvider;

    struct StubAiRunner(String);
    impl CommandRunner for StubAiRunner {
        fn run(
            &self,
            _program: &Path,
            _args: &[&str],
            _envs: &[(&str, &str)],
        ) -> std::io::Result<CommandOutput> {
            Ok(CommandOutput {
                status_code: 0,
                stdout: self.0.clone(),
                stderr: String::new(),
            })
        }
    }

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let fake_cli = temp.path().join("fake-ai");
    std::fs::write(&fake_cli, b"stub\n").expect("write stub ai");

    // 1. In-memory remote Linux snapshot analyzed locally via `analyze_remote_snapshot_with_runner`.
    let mut cache_dir = Node::directory(".cache");
    let mut pip_dir = Node::directory("pip");
    pip_dir
        .children
        .push(Node::entry("wheel.whl", NodeKind::File, 99_000));
    cache_dir.children.push(pip_dir);

    let mut local_dir = Node::directory(".local");
    let mut share_dir = Node::directory("share");
    let mut docker_dir = Node::directory("docker");
    docker_dir
        .children
        .push(Node::entry("layer.tar", NodeKind::File, 500_000));
    share_dir.children.push(docker_dir);
    local_dir.children.push(share_dir);

    let mut tree = Node::directory("deploy");
    tree.children.push(cache_dir);
    tree.children.push(local_dir);
    tree.children
        .push(Node::entry("sym", NodeKind::Symlink, 10_000));
    aggregate(&mut tree, Metric::Bytes);
    ruum_core::classify::classify(&mut tree);

    let mut header = Header::new(
        PathBuf::from("/home/deploy"),
        Some(PathBuf::from("/home/deploy")),
        "linux",
        Some(HostIdentity {
            hostname: "prod-01".into(),
            machine_id: Some("0123456789abcdef0123456789abcdef".into()),
        }),
    );
    let docker_summary = DockerUsageSummary {
        build_cache_reclaimable_bytes: 2_048_000,
        detailed: true,
        ..DockerUsageSummary::default()
    };
    header.docker = DockerProbeStatus::Available(docker_summary);

    let snapshot = Snapshot {
        docker: header.docker.clone(),
        header,
        tree,
    };
    let ai_cfg = AiConfig {
        provider: AiCliProvider::Custom,
        custom_command: fake_cli.to_string_lossy().into_owned(),
        ..AiConfig::default()
    };
    let runner = StubAiRunner(
        r#"{
            "summary": "Safe remote cache + build cache",
            "items": [
                {"path": "/home/deploy/.cache/pip", "mode": "permanent", "reason": "pip"},
                {"path": "/home/deploy/.local/share", "mode": "permanent", "reason": "docker ancestor"},
                {"path": "/home/deploy/sym", "mode": "permanent", "reason": "symlink"}
            ],
            "docker_targets": [{"kind": "build_cache"}]
        }"#
        .to_owned(),
    );

    let proposal = crate::state::analyze_remote_snapshot_with_runner(
        &ai_cfg,
        &snapshot,
        1_700_000_777,
        &runner,
    )
    .expect("remote snapshot analyzed locally");
    assert_eq!(proposal.items.len(), 1);
    assert_eq!(
        proposal.items[0].path,
        PathBuf::from("/home/deploy/.cache/pip")
    );
    assert_eq!(proposal.items[0].bytes, 99_000);
    assert_eq!(proposal.docker_targets, vec![DockerCleanTarget::BuildCache]);

    let req = crate::state::remote_proposal_confirm_request(
        snapshot.header.root,
        "prod-01",
        &proposal,
        crate::i18n::Locale::En,
    );
    assert!(req.title.contains("prod-01"));
    assert_eq!(req.items.len(), 1);

    // 2. GPUI `analyze_ssh_profile` surfaces `ssh_ai_analyze_failed` when
    //    credentials are missing without hanging or touching real SSH.
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let enc_path = cfg_dir.path().join("creds.enc");
    let counter = CountingRootSecret::new([0x44; 32]);
    let (view, cx) = view_over(temp.path(), cx);
    let profile = SshProfile {
        id: "srv-ai-err".into(),
        alias: "broken-box".into(),
        host: "127.0.0.1".into(),
        port: 22,
        username: "deploy".into(),
        auth_mode: AuthMode::Password,
        default_path: Some("~".into()),
        created_at_secs: 1,
        updated_at_secs: 1,
        last_total_bytes: None,
        last_available_bytes: None,
        last_scanned_secs: None,
        pending_script_path: None,
        pending_script_items: None,
        pending_reclaim_bytes: None,
        pending_created_secs: None,
    };
    update(&view, cx, |app, cx| {
        app.cred_store = Some(counter.store_for(&enc_path));
        app.ssh_profiles = vec![profile];
        app.analyze_ssh_profile("srv-ai-err", cx);
    });
    cx.run_until_parked();
    read(&view, cx, |app| {
        assert!(!app.ssh_analyzing_profiles.contains("srv-ai-err"));
        let msg = app
            .notice
            .as_ref()
            .map_or_else(String::new, |(text, _)| text.clone());
        assert!(msg.contains("broken-box"), "unexpected notice: {msg}");
    });
}

#[gpui_kit::test]
fn cleanup_confirm_window_docker_multiselect_defaults_and_docker_only_confirm(
    cx: &mut TestAppContext,
) {
    use ruum_core::confirm::CleanupConfirmDecision;
    use ruum_core::docker_adapter::{
        DockerCleanTarget, DockerContainerEntry, DockerImageEntry,
        DockerProbeStatus, DockerUsageSummary,
    };
    use ruum_core::watchdog_config::CleanupProposal;
    use std::sync::mpsc::channel;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (_, cx) = view_over(temp.path(), cx);

    let dangling_id = format!("sha256:{}", "1".repeat(64));
    let tagged_id = format!("sha256:{}", "2".repeat(64));
    let docker_summary = DockerUsageSummary {
        build_cache_reclaimable_bytes: 200_000_000,
        removable_images: vec![
            DockerImageEntry {
                id: dangling_id.clone(),
                repository: "<none>".into(),
                tag: "<none>".into(),
                size_bytes: 500_000_000,
                unique_bytes: 500_000_000,
                created_since: "2 weeks ago".into(),
                dangling: true,
            },
            DockerImageEntry {
                id: tagged_id.clone(),
                repository: "ghcr.io/demo/app".into(),
                tag: "v1.0".into(),
                size_bytes: 600_000_000,
                unique_bytes: 300_000_000,
                created_since: "5 days ago".into(),
                dangling: false,
            },
        ],
        stopped_containers: vec![DockerContainerEntry {
            id: "deadbeef1234".into(),
            name: "old-worker".into(),
            image: "ubuntu:22.04".into(),
            state: "exited".into(),
            status: "Exited (0) 3 days ago".into(),
            size_bytes: 100_000_000,
        }],
        detailed: true,
        ..DockerUsageSummary::default()
    };
    let docker_status = DockerProbeStatus::Available(docker_summary);

    // 0 file items, AI recommends the stopped container -> default checked:
    // BuildCache + dangling image + AI-recommended stopped container.
    let proposal = CleanupProposal {
        timestamp_secs: 1_700_000_000,
        summary: "Docker-only proposal".into(),
        items: Vec::new(),
        docker_targets: vec![DockerCleanTarget::Container {
            id: "deadbeef1234".into(),
            name: "old-worker".into(),
        }],
    };
    let req = crate::state::proposal_confirm_request_with_docker(
        temp.path().to_path_buf(),
        &proposal,
        &docker_status,
        crate::i18n::Locale::En,
    );
    assert!(req.items.is_empty());
    assert_eq!(req.docker_items.len(), 4);
    assert!(req.docker_items[0].default_checked);
    assert!(req.docker_items[1].default_checked);
    assert!(!req.docker_items[2].default_checked);
    assert_eq!(req.docker_items[2].reclaimable_bytes, 300_000_000);
    assert!(req.docker_items[3].default_checked);
    assert!(req.docker_items[3].reason.is_some());

    let (tx, rx) = channel();
    let handle = cx.update(|_, cx| {
        crate::open_confirm_window(req, Some(tx), None, cx)
            .expect("confirm window opened")
    });
    let mut confirm_cx = VisualTestContext::from_window(handle.into(), &cx.cx);
    draw(&mut confirm_cx);

    assert!(
        confirm_cx
            .debug_bounds("confirm-docker-group-cache")
            .is_some()
    );
    assert!(
        confirm_cx
            .debug_bounds("confirm-docker-group-images")
            .is_some()
    );
    assert!(
        confirm_cx
            .debug_bounds("confirm-docker-group-containers")
            .is_some()
    );

    // Uncheck BuildCache (idx 0), check tagged image (idx 2).
    let cache_cb = confirm_cx
        .debug_bounds("confirm-docker-check-0")
        .expect("build cache checkbox");
    confirm_cx.simulate_click(cache_cb.center(), gpui_kit::Modifiers::none());
    let tagged_cb = confirm_cx
        .debug_bounds("confirm-docker-check-2")
        .expect("tagged image checkbox");
    confirm_cx.simulate_click(tagged_cb.center(), gpui_kit::Modifiers::none());
    draw(&mut confirm_cx);

    // Confirm with 0 files + 3 Docker targets.
    let confirm_btn = confirm_cx
        .debug_bounds("confirm-clean")
        .expect("confirm button");
    confirm_cx
        .simulate_click(confirm_btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();

    assert_eq!(
        rx.recv().expect("docker-only confirm decision"),
        CleanupConfirmDecision::Confirmed {
            selected_paths: Vec::new(),
            selected_docker_targets: vec![
                DockerCleanTarget::Image {
                    id: dangling_id,
                    reference: None,
                    dangling: true,
                },
                DockerCleanTarget::Image {
                    id: tagged_id,
                    reference: Some("ghcr.io/demo/app:v1.0".into()),
                    dangling: false,
                },
                DockerCleanTarget::Container {
                    id: "deadbeef1234".into(),
                    name: "old-worker".into(),
                },
            ],
        }
    );

    // PermissionDenied renders read-only hint banner.
    let denied_req = crate::state::proposal_confirm_request_with_docker(
        temp.path().to_path_buf(),
        &CleanupProposal::default(),
        &DockerProbeStatus::PermissionDenied {
            socket_hint: "/var/run/docker.sock".into(),
        },
        crate::i18n::Locale::En,
    );
    let denied_handle = cx.update(|_, cx| {
        crate::open_confirm_window(denied_req, None, None, cx)
            .expect("denied confirm window")
    });
    let mut denied_cx =
        VisualTestContext::from_window(denied_handle.into(), &cx.cx);
    draw(&mut denied_cx);
    assert!(
        denied_cx
            .debug_bounds("confirm-docker-permission-denied")
            .is_some()
    );
}

#[gpui_kit::test]
fn local_docker_workspace_batch_cleanup_and_done_routing(
    cx: &mut TestAppContext,
) {
    use ruum_core::docker_adapter::{
        DockerCleanTarget, DockerCommandOutput, DockerCommandRunner,
        DockerContainerEntry, DockerImageEntry, DockerProbeStatus,
        DockerUsageSummary,
    };
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Debug, Default)]
    struct MockDockerRunner {
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl DockerCommandRunner for MockDockerRunner {
        fn run(
            &self,
            _program: &Path,
            args: &[&str],
            _timeout: Duration,
        ) -> std::io::Result<DockerCommandOutput> {
            self.calls
                .lock()
                .expect("lock calls")
                .push(args.iter().map(|s| (*s).to_string()).collect());
            if args.first().copied() == Some("system") {
                return Ok(DockerCommandOutput {
                    status_code: 0,
                    stdout: r#"{"Images":[],"Containers":[],"Volumes":[],"BuildCache":[]}"#.into(),
                    stderr: String::new(),
                });
            }
            if args.starts_with(&["image", "rm", "--"]) {
                if args.len() > 4 {
                    return Ok(DockerCommandOutput {
                        status_code: 1,
                        stdout: String::new(),
                        stderr: "batch image failure".into(),
                    });
                }
                if args.get(3).copied() == Some("ghcr.io/demo/app:v1.0") {
                    return Ok(DockerCommandOutput {
                        status_code: 1,
                        stdout: String::new(),
                        stderr: "image is referenced by manifest".into(),
                    });
                }
            }
            Ok(DockerCommandOutput {
                status_code: 0,
                stdout: "Total reclaimed space: 100MB\n".into(),
                stderr: String::new(),
            })
        }
    }

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    let runner = Arc::new(MockDockerRunner::default());

    let dangling_id = format!("sha256:{}", "a".repeat(64));
    let tagged_id = format!("sha256:{}", "b".repeat(64));
    let untouched_id = format!("sha256:{}", "c".repeat(64));
    let summary = DockerUsageSummary {
        build_cache_reclaimable_bytes: 120_000_000,
        build_cache_shared_bytes: 40_000_000,
        removable_images: vec![
            DockerImageEntry {
                id: dangling_id.clone(),
                repository: "<none>".into(),
                tag: "<none>".into(),
                size_bytes: 80_000_000,
                unique_bytes: 80_000_000,
                created_since: "3 days ago".into(),
                dangling: true,
            },
            DockerImageEntry {
                id: tagged_id,
                repository: "ghcr.io/demo/app".into(),
                tag: "v1.0".into(),
                size_bytes: 10_000_000,
                unique_bytes: 7_000_000,
                created_since: "10 days ago".into(),
                dangling: false,
            },
            DockerImageEntry {
                id: untouched_id.clone(),
                repository: "alpine".into(),
                tag: "3.19".into(),
                size_bytes: 5_000_000,
                unique_bytes: 5_000_000,
                created_since: "12 days ago".into(),
                dangling: false,
            },
        ],
        stopped_containers: vec![DockerContainerEntry {
            id: "c123456789ab".into(),
            name: "stopped-job".into(),
            image: "ghcr.io/demo/app:v1.0".into(),
            state: "exited".into(),
            status: "Exited (1) 2 days ago".into(),
            size_bytes: 5_000_000,
        }],
        detailed: true,
        ..DockerUsageSummary::default()
    };

    update(&view, cx, |app, cx| {
        let runner_concrete = Arc::clone(&runner);
        let dyn_runner: Arc<dyn DockerCommandRunner> = runner_concrete;
        app.docker_runner = Some(dyn_runner);
        app.apply_local_docker_status(DockerProbeStatus::Available(summary));
        cx.notify();
    });
    draw(cx);

    // 1. With 0 file marks on Explore, Review stays hidden and `c` does not
    //    leave Explore even though Docker has cleanable items.
    assert!(cx.debug_bounds("review").is_none());
    assert!(cx.debug_bounds("docker-status-line").is_none());
    press(cx, "c");
    draw(cx);
    assert_eq!(read(&view, cx, |a| a.screen), Screen::Explore);

    // 2. Navigate to dedicated Docker page via rail icon.
    let docker_rail = cx.debug_bounds("rail-docker").expect("rail-docker");
    cx.simulate_click(docker_rail.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(read(&view, cx, |a| a.screen), Screen::Docker);
    assert!(cx.debug_bounds("docker-page").is_some());
    assert!(cx.debug_bounds("docker-build-cache-shared").is_some());
    assert!(cx.debug_bounds("docker-build-cache").is_some());
    assert!(cx.debug_bounds("docker-image-0").is_some());
    assert!(cx.debug_bounds("docker-image-1").is_some());
    assert!(cx.debug_bounds("docker-image-2").is_some());
    assert!(cx.debug_bounds("docker-container-0").is_some());

    // Default checked: BuildCache + dangling image.
    let expected_dangling = dangling_id;
    read(&view, cx, |a| {
        assert_eq!(
            a.selected_local_docker_targets(),
            vec![
                DockerCleanTarget::Image {
                    id: expected_dangling,
                    reference: None,
                    dangling: true,
                },
                DockerCleanTarget::BuildCache,
            ]
        );
    });

    // Check stopped container + slash-containing image `ghcr.io/demo/app:v1.0`.
    let cont_cb = cx
        .debug_bounds("docker-container-0")
        .expect("stopped container checkbox");
    cx.simulate_click(cont_cb.center(), gpui_kit::Modifiers::none());
    draw(cx);
    let img_cb = cx
        .debug_bounds("docker-image-1")
        .expect("tagged image checkbox");
    cx.simulate_click(img_cb.center(), gpui_kit::Modifiers::none());
    draw(cx);

    let commit_btn = cx
        .debug_bounds("docker-commit")
        .expect("docker-commit button");
    cx.simulate_click(commit_btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    draw(cx);

    // 3. Done shows outcomes without fake treemap rescan note, preserving full
    //    Docker image path with slashes.
    assert_eq!(read(&view, cx, |a| a.screen), Screen::Done);
    assert!(cx.debug_bounds("done-rescan-note").is_none());
    read(&view, cx, |a| {
        assert_eq!(a.run_summary.removed, 3);
        assert_eq!(a.run_summary.failed, 1);
        assert!(
            a.run_log.iter().any(|(path, res)| {
                path == &PathBuf::from("docker: image ghcr.io/demo/app:v1.0")
                    && res.is_err()
            }),
            "expected full docker image failure label in {:?}",
            a.run_log
        );
        assert!(
            a.run_log.iter().any(|(path, res)| {
                path == &PathBuf::from("docker: build cache") && res.is_ok()
            }),
            "expected build cache log in {:?}",
            a.run_log
        );
    });

    let calls = runner.calls.lock().expect("lock calls").clone();
    let c_idx = calls
        .iter()
        .position(|c| c == &["container", "rm", "--", "c123456789ab"])
        .expect("container rm call");
    let i_batch_idx = calls
        .iter()
        .position(|c| {
            c.starts_with(&[
                "image".to_owned(),
                "rm".to_owned(),
                "--".to_owned(),
            ]) && c.len() == 5
        })
        .expect("batched image rm call");
    let bc_idx = calls
        .iter()
        .position(|c| c == &["builder", "prune", "-f"])
        .expect("builder prune call");
    assert!(c_idx < i_batch_idx && i_batch_idx < bc_idx);
    assert!(
        !calls.iter().any(|c| c
            .iter()
            .any(|arg| arg == &untouched_id || arg == "alpine:3.19")),
        "unchecked alpine:3.19 must not be removed: {calls:?}"
    );

    // 4. Enter on Done returns to Screen::Docker and resets return target.
    press(cx, "enter");
    draw(cx);
    read(&view, cx, |a| {
        assert_eq!(a.screen, Screen::Docker);
        assert_eq!(a.done_return_screen, Screen::Explore);
    });

    // 5. Clicking Refresh on Screen::Docker triggers probe via mock runner.
    let refresh_btn = cx
        .debug_bounds("docker-refresh")
        .expect("docker-refresh button");
    cx.simulate_click(refresh_btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    let calls_after = runner.calls.lock().expect("lock calls").clone();
    assert!(
        calls_after
            .iter()
            .filter(|c| c.first().map(String::as_str) == Some("system"))
            .count()
            >= 2
    );
}

#[gpui_kit::test]
fn remote_review_and_confirm_generate_script_dialog_and_pending_card_actions(
    cx: &mut TestAppContext,
) {
    use ruum_core::credentials::SshCredentialSecret;
    use ruum_core::docker_adapter::{
        DockerCleanTarget, DockerProbeStatus, DockerUsageSummary,
    };
    use ruum_core::identity::HostIdentity;
    use ruum_core::remote::Header;
    use ruum_core::ssh_session::{SshAuthMaterial, SshEndpoint, SshSession};
    use ruum_core::storage::{AuthMode, SshProfile, StorageDb};
    use ruum_core::terminal_launcher::{TerminalEnv, TerminalOs};
    use ruum_core::watchdog_config::CleanupProposal;
    use std::sync::Arc;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let db_dir = tempfile::tempdir().expect("db dir");
    let db_path = db_dir.path().join("ruum.db");
    let fake_kitty = db_dir.path().join("kitty");
    let fake_ruum = db_dir.path().join("ruum");
    std::fs::write(&fake_kitty, b"#!/bin/sh\n").expect("write fake kitty");
    std::fs::write(&fake_ruum, b"#!/bin/sh\n").expect("write fake ruum");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        for bin in [&fake_kitty, &fake_ruum] {
            let mut perms = std::fs::metadata(bin).expect("meta").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(bin, perms).expect("chmod");
        }
    }

    let profile = SshProfile {
        id: "srv-remote-s4".into(),
        alias: "prod-s4".into(),
        host: "10.0.0.42".into(),
        port: 2222,
        username: "deploy".into(),
        auth_mode: AuthMode::PrivateKey,
        default_path: Some("/home/deploy".into()),
        last_total_bytes: None,
        last_available_bytes: None,
        last_scanned_secs: None,
        pending_script_path: None,
        pending_script_items: None,
        pending_reclaim_bytes: None,
        pending_created_secs: None,
        created_at_secs: 1,
        updated_at_secs: 1,
    };
    let saved_profile = {
        let db = StorageDb::open(&db_path).expect("open db");
        db.add_ssh_profile(&profile).expect("save profile")
    };
    let profile_id = saved_profile.id.clone();

    let (view, cx) = view_over(temp.path(), cx);
    let mut header = Header::new(
        PathBuf::from("/home/deploy"),
        Some(PathBuf::from("/home/deploy")),
        "linux",
        Some(HostIdentity {
            hostname: "prod-s4".into(),
            machine_id: Some("00112233445566778899aabbccddeeff".into()),
        }),
    );
    header.docker = DockerProbeStatus::Available(DockerUsageSummary {
        build_cache_reclaimable_bytes: 400_000_000,
        detailed: true,
        ..DockerUsageSummary::default()
    });

    let terminal_env = TerminalEnv {
        current_exe: db_dir.path().join("ruum-gui"),
        path_var: Some(db_dir.path().as_os_str().to_os_string()),
        terminal_var: None,
        local_appdata: None,
    };

    let pid_for_setup = profile_id.clone();
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.reload_ssh_profiles();
        app.root_path = header.root.clone();
        app.home = header.home.clone();
        app.remote = Some("prod-s4".into());
        app.remote_identity = header.identity.clone();
        app.remote_header = Some(header.clone());
        app.active_ssh_profile_id = Some(pid_for_setup);
        app.docker = header.docker.clone();
        app.terminal_os_override = Some(TerminalOs::Linux);
        app.terminal_env_override = Some(terminal_env);
        app.marks.toggle(ruum_core::removal::Target {
            path: PathBuf::from("/home/deploy/.cache/pip"),
            bytes: 15_000_000,
            is_dir: true,
            hidden: false,
        });
        app.open_review();
        cx.notify();
    });
    draw(cx);

    assert!(cx.debug_bounds("generate-remote-script").is_some());
    assert!(cx.debug_bounds("review-docker-panel").is_none());
    assert!(cx.debug_bounds("review-docker-build-cache").is_none());

    // 1. Remote Review primary button generates files-only `.sh`, saves DB
    //    pending state, and displays the generated-script modal.
    let gen_btn = cx
        .debug_bounds("generate-remote-script")
        .expect("generate remote script button");
    cx.simulate_click(gen_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);

    assert!(cx.debug_bounds("script-generated-dialog").is_some());
    let script_path = read(&view, cx, |a| {
        a.generated_script_dialog
            .as_ref()
            .expect("dialog open")
            .script_path
            .clone()
    });
    assert!(script_path.exists());
    let script_text =
        std::fs::read_to_string(&script_path).expect("read generated script");
    assert!(script_text.contains("remove '/home/deploy/.cache/pip'"));
    assert!(!script_text.contains("docker 'builder' 'prune' '-f'"));

    // Click "View Script File", then "Run in Terminal Now".
    let reveal_btn = cx
        .debug_bounds("script-dialog-reveal")
        .expect("reveal script button");
    cx.simulate_click(reveal_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    let run_btn = cx
        .debug_bounds("script-dialog-run-terminal")
        .expect("run in terminal button");
    cx.simulate_click(run_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);

    let pid_for_assert = profile_id.clone();
    read(&view, cx, |a| {
        assert!(a.generated_script_dialog.is_none());
        assert_eq!(a.terminal_launch_log.len(), 2);
        assert_eq!(a.terminal_launch_log[0].0, "reveal");
        assert_eq!(a.terminal_launch_log[1].0, "run-script");
        assert_eq!(a.terminal_launch_log[1].1.program, fake_kitty);
        assert!(
            a.terminal_launch_log[1]
                .1
                .args
                .iter()
                .any(|arg| arg == pid_for_assert.as_str())
        );
    });

    // 1b. Remote Docker modal in Explore generates Docker-only `.sh` script
    //     and opens dialog while `rail-docker` remains hidden (local NotInstalled).
    press(cx, "escape");
    draw(cx);
    assert_eq!(read(&view, cx, |a| a.screen), Screen::Explore);
    assert!(cx.debug_bounds("rail-docker").is_none());
    let remote_docker_btn = cx
        .debug_bounds("remote-docker-btn")
        .expect("remote-docker-btn in Explore toolbar");
    cx.simulate_click(remote_docker_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(cx.debug_bounds("remote-docker-modal").is_some());
    assert!(cx.debug_bounds("docker-build-cache").is_some());
    let docker_gen_btn = cx
        .debug_bounds("docker-generate-script")
        .expect("docker generate script button");
    cx.simulate_click(docker_gen_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(cx.debug_bounds("remote-docker-modal").is_none());
    assert!(cx.debug_bounds("script-generated-dialog").is_some());
    let docker_script_path = read(&view, cx, |a| {
        a.generated_script_dialog
            .as_ref()
            .expect("docker script dialog open")
            .script_path
            .clone()
    });
    let docker_script_text = std::fs::read_to_string(&docker_script_path)
        .expect("read docker remote script");
    assert!(docker_script_text.contains("docker 'builder' 'prune' '-f'"));
    assert!(!docker_script_text.contains("remove '/home/deploy/.cache/pip'"));
    let close_dialog = cx.debug_bounds("script-dialog-later");
    if let Some(close_btn) = close_dialog {
        cx.simulate_click(close_btn.center(), gpui_kit::Modifiers::none());
    } else {
        update(&view, cx, |app, cx| {
            app.generated_script_dialog = None;
            cx.notify();
        });
    }
    draw(cx);

    // 2. Navigate to Remote -> SSH card renders pending banner with
    //    Run / View / Discard buttons; Discard clears DB & hides banner.
    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Remote, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("ssh-pending-banner-0").is_some());
    assert!(cx.debug_bounds("ssh-pending-run-0").is_some());
    assert!(cx.debug_bounds("ssh-pending-view-0").is_some());
    let discard_btn = cx
        .debug_bounds("ssh-pending-discard-0")
        .expect("discard pending script button");
    cx.simulate_click(discard_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);

    assert!(cx.debug_bounds("ssh-pending-banner-0").is_none());
    let db = StorageDb::open(&db_path).expect("reopen db");
    let reloaded = db.list_ssh_profiles().expect("load profiles");
    assert!(reloaded[0].pending_script_path.is_none());

    // 3. Remote CleanupConfirmWindow confirmation generates script and pops
    //    `script-generated-dialog` on the main window.
    let endpoint = SshEndpoint::from_profile(&saved_profile).expect("endpoint");
    let material = SshAuthMaterial::from_credential_secret(
        &SshCredentialSecret::Password {
            password: "dummy-password".into(),
        },
    );
    let session = Arc::new(
        SshSession::start(endpoint, material, 15).expect("start session"),
    );
    let remote_proposal = CleanupProposal {
        timestamp_secs: 1_700_000_001,
        summary: "Remote docker cache".into(),
        items: Vec::new(),
        docker_targets: vec![DockerCleanTarget::BuildCache],
    };
    let req = crate::state::remote_proposal_confirm_request_with_docker(
        header.root.clone(),
        "prod-s4",
        &remote_proposal,
        &header.docker,
        crate::i18n::Locale::En,
    );
    let confirm_win = cx
        .update(|_, cx| {
            crate::open_remote_confirm_window(
                req,
                profile_id,
                session,
                header,
                Some(db_path.clone()),
                cx,
            )
        })
        .expect("open remote confirm window");
    let mut confirm_cx =
        VisualTestContext::from_window(confirm_win.into(), &cx.cx);
    draw(&mut confirm_cx);
    let confirm_clean = confirm_cx
        .debug_bounds("confirm-clean")
        .expect("remote confirm clean");
    confirm_cx
        .simulate_click(confirm_clean.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    draw(cx);

    assert!(cx.debug_bounds("script-generated-dialog").is_some());
    let db_after = StorageDb::open(&db_path).expect("db after confirm");
    let profiles_after = db_after.list_ssh_profiles().expect("load after");
    assert!(profiles_after[0].pending_script_path.is_some());
}

#[test]
fn preload_credentials_if_needed_requires_profiles_and_encrypted_file() {
    use ruum_core::credentials::CREDENTIALS_FILE_NAME;
    use ruum_core::storage::{AuthMode, SshProfile, StorageDb};

    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("ruum.db");
    let enc_path = dir.path().join(CREDENTIALS_FILE_NAME);
    let counter = CountingRootSecret::new([0x51; 32]);
    let store = counter.store_for(&enc_path);

    // 1. None or missing DB path -> Ok(false), 0 keychain calls, never creates DB.
    assert_eq!(
        crate::preload_credentials_if_needed(None, &store),
        Ok(false)
    );
    assert_eq!(
        crate::preload_credentials_if_needed(Some(&db_path), &store),
        Ok(false)
    );
    assert!(!db_path.exists());
    assert_eq!(counter.load_count(), 0);

    // 2. Existing DB with 0 SSH profiles + existing encrypted file -> Ok(false),
    //    0 keychain calls.
    let writer = CountingRootSecret::new([0x51; 32]).store_for(&enc_path);
    writer.set("ruum.ssh", "orphan", b"pw").expect("seed enc");
    {
        let _db = StorageDb::open(&db_path).expect("create db");
    }
    assert_eq!(
        crate::preload_credentials_if_needed(Some(&db_path), &store),
        Ok(false)
    );
    assert_eq!(counter.load_count(), 0);

    // 3. Existing DB with SSH profile + missing encrypted file -> Ok(false),
    //    0 keychain calls.
    std::fs::remove_file(&enc_path).expect("remove enc");
    {
        let db = StorageDb::open(&db_path).expect("open db");
        db.add_ssh_profile(&SshProfile::new(
            "prod",
            "10.0.0.10",
            22,
            "ubuntu",
            AuthMode::Password,
        ))
        .expect("add profile");
    }
    assert_eq!(
        crate::preload_credentials_if_needed(Some(&db_path), &store),
        Ok(false)
    );
    assert_eq!(counter.load_count(), 0);

    // 4. Existing DB with SSH profile + existing encrypted file -> preloads
    //    once, then repeats are no-ops.
    writer
        .set("ruum.ssh", "prod", b"secret")
        .expect("re-seed enc");
    assert_eq!(
        crate::preload_credentials_if_needed(Some(&db_path), &store),
        Ok(true)
    );
    assert!(store.is_loaded());
    assert_eq!(counter.load_count(), 1);
    assert_eq!(
        crate::preload_credentials_if_needed(Some(&db_path), &store),
        Ok(false)
    );
    assert_eq!(counter.load_count(), 1);
}

#[gpui_kit::test]
fn ensure_cred_store_caches_and_reuses_shared_handle_across_reopens(
    cx: &mut TestAppContext,
) {
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let enc_path = cfg_dir.path().join("device-credentials.enc");
    let counter = CountingRootSecret::new([0x66; 32]);
    let shared_store = counter.store_for(&enc_path);
    shared_store
        .set("ruum.ssh", "prof-shared", b"shared-pw")
        .expect("seed shared secret");
    assert_eq!(counter.load_count(), 1);

    // 1. Open main window #1 via AppRuntime + open_or_navigate_main_window:
    //    `Ruum` inherits `AppRuntime.cred_store` automatically with 0 manual
    //    field assignment and 0 extra keychain calls.
    cx.update(|cx| {
        gpui_omarchy::init(cx);
        crate::install_notification_handler(cx);
        crate::install_test_runtime(temp.path().to_path_buf(), false, None, cx);
        crate::with_runtime_mut(cx, |rt| {
            rt.cred_store = Some(shared_store.clone());
        });
        crate::open_or_navigate_main_window(None, cx);
    });
    cx.run_until_parked();

    let win1 = cx
        .read(|cx| cx.global::<crate::AppRuntime>().main_window)
        .expect("win1 opened via AppRuntime");
    let val1 = win1
        .update(cx, |app, _, _| {
            assert!(app.cred_store.is_some());
            let store = app.ensure_cred_store().expect("win1 cred store");
            store.get("ruum.ssh", "prof-shared").expect("get win1")
        })
        .expect("win1 update");
    assert_eq!(val1.unwrap().as_slice(), b"shared-pw");
    assert_eq!(counter.load_count(), 1);

    // 2. Close main window #1 (tray-resident state) and reopen main window #2:
    //    reopened window inherits the same `AppRuntime.cred_store` handle.
    win1.update(cx, |_, window, cx| {
        window.remove_window();
        crate::with_runtime_mut(cx, |rt| rt.main_window = None);
    })
    .expect("close win1");
    cx.run_until_parked();

    cx.update(|cx| {
        crate::open_or_navigate_main_window(None, cx);
    });
    cx.run_until_parked();

    let win2 = cx
        .read(|cx| cx.global::<crate::AppRuntime>().main_window)
        .expect("win2 reopened via AppRuntime");
    assert_ne!(win1, win2);
    let val2 = win2
        .update(cx, |app, _, _| {
            assert!(app.cred_store.is_some());
            let store = app.ensure_cred_store().expect("win2 cred store");
            store.get("ruum.ssh", "prof-shared").expect("get win2")
        })
        .expect("win2 update");
    assert_eq!(val2.unwrap().as_slice(), b"shared-pw");
    assert_eq!(counter.load_count(), 1);

    // 3. When `Ruum::cred_store` starts `None` with `db_path` set,
    //    `ensure_cred_store` lazily constructs and caches it on `self`.
    let fallback_win =
        cx.add_window(|_, cx| Ruum::choose(options(), 3, None, cx));
    fallback_win
        .update(cx, |app, _, _| {
            assert!(app.cred_store.is_none());
            app.db_path = Some(db_path);
            let created = app.ensure_cred_store().expect("create fallback");
            assert!(app.cred_store.is_some());
            assert_eq!(
                created.file_path(),
                app.cred_store.as_ref().unwrap().file_path()
            );
        })
        .expect("fallback update");
}

#[gpui_kit::test]
fn pending_open_docker_navigates_only_when_first_probe_is_available(
    cx: &mut TestAppContext,
) {
    use ruum_core::docker_adapter::{DockerProbeStatus, DockerUsageSummary};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);

    // 1. First probe arrives Available while `pending_open_docker` is armed ->
    //    jumps to Screen::Docker and clears flag.
    update(&view, cx, |app, cx| {
        app.pending_open_docker = true;
        app.apply_local_docker_status(DockerProbeStatus::Available(
            DockerUsageSummary::default(),
        ));
        cx.notify();
    });
    draw(cx);
    read(&view, cx, |app| {
        assert_eq!(app.screen, Screen::Docker);
        assert!(!app.pending_open_docker);
    });

    // 2. First probe arrives DaemonStopped while `pending_open_docker` is
    //    armed -> stays on Explore, clears flag so a later Available does not
    //    hijack the active view.
    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Explore, cx);
        app.pending_open_docker = true;
        app.apply_local_docker_status(DockerProbeStatus::DaemonStopped);
        cx.notify();
    });
    read(&view, cx, |app| {
        assert_eq!(app.screen, Screen::Explore);
        assert!(!app.pending_open_docker);
    });
    update(&view, cx, |app, cx| {
        app.apply_local_docker_status(DockerProbeStatus::Available(
            DockerUsageSummary::default(),
        ));
        cx.notify();
    });
    read(&view, cx, |app| {
        assert_eq!(app.screen, Screen::Explore);
    });
}

#[gpui_kit::test]
fn ssd_mode_omits_csd_title_bar_and_f11_toggles_fullscreen(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    cx.update(|_, cx| {
        crate::app_menu::install_for(crate::i18n::Locale::En, cx);
        crate::app_menu::set_tray_active(cx, true);
    });
    draw(cx);

    // Default test window reports Decorations::Server -> no CSD chrome.
    assert!(cx.debug_bounds("csd-title-bar").is_none());
    assert!(cx.debug_bounds("csd-window-backdrop").is_none());
    assert!(cx.debug_bounds("ruum-root").is_some());

    // ToggleFullscreen action toggles fullscreen without panicking.
    let initial_fs = cx.update(|window, _| window.is_fullscreen());
    assert!(!initial_fs);
    cx.dispatch_action(crate::app_menu::ToggleFullscreen);
    draw(cx);
    assert!(cx.update(|window, _| window.is_fullscreen()));
    cx.dispatch_action(crate::app_menu::ToggleFullscreen);
    draw(cx);
    assert!(!cx.update(|window, _| window.is_fullscreen()));

    // On non-macOS platforms, F11 is bound to ToggleFullscreen.
    if !cfg!(target_os = "macos") {
        let focus = read(&view, cx, |app| app.focus.clone());
        cx.update(|window, cx| window.focus(&focus, cx));
        press(cx, "f11");
        assert!(cx.update(|window, _| window.is_fullscreen()));
        press(cx, "f11");
        assert!(!cx.update(|window, _| window.is_fullscreen()));
    }
}

#[gpui_kit::test]
fn confirm_windows_close_window_action_cancels_and_denies_safely(
    cx: &mut TestAppContext,
) {
    use ruum_core::confirm::{
        CleanupConfirmDecision, CleanupConfirmItem, CleanupConfirmRequest,
        RemoteConnectConfirmDecision, RemoteConnectConfirmRequest,
    };
    use std::sync::mpsc::channel;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (_, cx) = view_over(temp.path(), cx);
    cx.update(|_, cx| {
        crate::app_menu::install_for(crate::i18n::Locale::En, cx);
    });

    // 1. CleanupConfirmWindow: CloseWindow action invokes `cancel`.
    let req = CleanupConfirmRequest {
        root: temp.path().to_path_buf(),
        title: "Close Action Confirm".into(),
        summary: Some("Cancel via CloseWindow".into()),
        items: vec![CleanupConfirmItem {
            path: temp.path().join("junk/blob.bin"),
            bytes: 200_000,
            mode: RemovalMode::Trash,
            reason: None,
            git_warning: None,
        }],
        docker_items: Vec::new(),
        docker_permission_hint: None,
    };
    let (tx_clean, rx_clean) = channel();
    let clean_handle = cx.update(|_, cx| {
        crate::open_confirm_window(req, Some(tx_clean), None, cx)
            .expect("cleanup confirm window")
    });
    let mut clean_cx =
        VisualTestContext::from_window(clean_handle.into(), &cx.cx);
    draw(&mut clean_cx);
    assert!(clean_cx.debug_bounds("csd-title-bar").is_none());
    assert!(clean_cx.debug_bounds("confirm-cancel").is_some());
    assert!(clean_cx.debug_bounds("confirm-clean").is_some());

    clean_cx.dispatch_action(crate::app_menu::CloseWindow);
    cx.run_until_parked();
    assert_eq!(rx_clean.try_recv(), Ok(CleanupConfirmDecision::Cancelled));
    assert_eq!(cx.windows().len(), 1);

    // 2. RemoteConnectConfirmWindow: CloseWindow action invokes `deny`.
    let remote_req = RemoteConnectConfirmRequest {
        target_label: "prod".into(),
        alias: Some("prod".into()),
        host: "10.0.0.1".into(),
        port: 22,
        username: "deploy".into(),
        auth_mode: "private_key".into(),
        tool_name: "analyze_remote_disk".into(),
    };
    let (tx_remote, rx_remote) = channel();
    let remote_handle = cx.update(|_, cx| {
        crate::open_remote_connect_confirm_window_with_mode(
            remote_req,
            Some(tx_remote),
            None,
            false,
            cx,
        )
        .expect("remote connect confirm window")
    });
    let mut remote_cx =
        VisualTestContext::from_window(remote_handle.into(), &cx.cx);
    draw(&mut remote_cx);
    assert!(remote_cx.debug_bounds("csd-title-bar").is_none());
    assert!(remote_cx.debug_bounds("remote-connect-deny").is_some());
    assert!(remote_cx.debug_bounds("remote-connect-approve").is_some());

    remote_cx.dispatch_action(crate::app_menu::CloseWindow);
    cx.run_until_parked();
    assert_eq!(
        rx_remote.try_recv(),
        Ok(RemoteConnectConfirmDecision::Denied)
    );
    assert_eq!(cx.windows().len(), 1);
}

#[gpui_kit::test]
fn injected_csd_chrome_renders_controls_layout_drag_resize_and_tiling(
    cx: &mut TestAppContext,
) {
    use gpui_kit::{
        Decorations, InteractiveElement as _, MAX_BUTTONS_PER_SIDE, Modifiers,
        MouseButton, MouseDownEvent, MouseUpEvent, Render, ResizeEdge,
        Styled as _, Tiling, WindowButton, WindowButtonLayout, WindowControls,
        div, point, size,
    };
    use gpui_omarchy::IconName;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct ChromeCalls {
        minimize: usize,
        zoom: usize,
        close: usize,
        start_move: usize,
        resizes: Vec<ResizeEdge>,
        menus: Vec<Point<Pixels>>,
    }

    struct CsdTestHost {
        state: crate::window_chrome::WindowChromeState,
        calls: Rc<RefCell<ChromeCalls>>,
    }

    impl Render for CsdTestHost {
        fn render(
            &mut self,
            window: &mut gpui_kit::Window,
            _: &mut Context<'_, Self>,
        ) -> impl gpui_kit::IntoElement {
            let mut state = self.state;
            state.window_size = window.bounds().size;
            let c_min = Rc::clone(&self.calls);
            let c_zoom = Rc::clone(&self.calls);
            let c_close = Rc::clone(&self.calls);
            let c_move = Rc::clone(&self.calls);
            let c_resize = Rc::clone(&self.calls);
            let c_menu = Rc::clone(&self.calls);
            let actions = crate::window_chrome::WindowChromeActions {
                start_resize: Rc::new(move |edge, _, _| {
                    c_resize.borrow_mut().resizes.push(edge);
                }),
                start_move: Rc::new(move |_, _| {
                    c_move.borrow_mut().start_move += 1;
                }),
                zoom: Rc::new(move |_, _| {
                    c_zoom.borrow_mut().zoom += 1;
                }),
                minimize: Rc::new(move |_, _| {
                    c_min.borrow_mut().minimize += 1;
                }),
                show_window_menu: Rc::new(move |pos, _, _| {
                    c_menu.borrow_mut().menus.push(pos);
                }),
                close: Rc::new(move |_, _| {
                    c_close.borrow_mut().close += 1;
                }),
            };
            let body = div()
                .id("csd-body-marker")
                .debug_selector(|| "csd-body-marker".into())
                .flex_1()
                .min_h_0()
                .min_w_0()
                .size_full();
            crate::window_chrome::wrap_window_chrome_with(
                "ruum · ~/Projects",
                body,
                state,
                actions,
            )
        }
    }

    cx.update(gpui_omarchy::init);
    let calls = Rc::new(RefCell::new(ChromeCalls::default()));
    let calls_ref = Rc::clone(&calls);
    let initial_state = crate::window_chrome::WindowChromeState {
        decorations: Decorations::Client {
            tiling: Tiling::default(),
        },
        controls: WindowControls::default(),
        button_layout: crate::window_chrome::default_linux_button_layout(),
        is_maximized: false,
        is_window_active: true,
        window_size: size(px(800.0), px(600.0)),
    };
    let (view, cx) = cx.add_window_view(move |_, _| CsdTestHost {
        state: initial_state,
        calls: calls_ref,
    });
    cx.simulate_resize(size(px(800.0), px(600.0)));
    draw(cx);

    assert_eq!(
        cx.update(|window, _| window.client_inset()),
        Some(crate::window_chrome::CSD_SHADOW_SIZE)
    );
    let title_bar = cx.debug_bounds("csd-title-bar").expect("csd-title-bar");
    let title_text = cx.debug_bounds("csd-title-text").expect("csd-title-text");
    assert!(cx.debug_bounds("csd-controls-left").is_none());
    assert!(cx.debug_bounds("csd-controls-right").is_some());
    let btn_min = cx.debug_bounds("csd-btn-minimize").expect("min button");
    let btn_max = cx.debug_bounds("csd-btn-maximize").expect("max button");
    let btn_close = cx.debug_bounds("csd-btn-close").expect("close button");
    assert!(btn_min.origin.x < btn_max.origin.x);
    assert!(btn_max.origin.x < btn_close.origin.x);
    assert!(title_text.right() <= btn_min.left());

    // Clicking titlebar buttons fires each action without starting window drag.
    cx.simulate_click(btn_min.center(), Modifiers::none());
    cx.simulate_click(btn_max.center(), Modifiers::none());
    cx.simulate_click(btn_close.center(), Modifiers::none());
    {
        let c = calls.borrow();
        assert_eq!(c.minimize, 1);
        assert_eq!(c.zoom, 1);
        assert_eq!(c.close, 1);
        assert_eq!(c.start_move, 0);
    }

    // Dragging empty title text starts window move.
    let drag_start = title_text.center();
    cx.simulate_mouse_down(drag_start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(drag_start.x + px(12.0), drag_start.y + px(2.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        point(drag_start.x + px(12.0), drag_start.y + px(2.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert_eq!(calls.borrow().start_move, 1);

    // Double-clicking empty title text zooms without triggering move.
    cx.simulate_event(MouseDownEvent {
        position: drag_start,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        position: drag_start,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
    });
    assert_eq!(calls.borrow().zoom, 2);
    assert_eq!(calls.borrow().start_move, 1);

    // Right-clicking title bar shows window menu only when controls allow it.
    cx.simulate_mouse_down(drag_start, MouseButton::Right, Modifiers::none());
    assert_eq!(calls.borrow().menus, vec![drag_start]);

    // Left mouse down on top-left and right frame edges starts resize.
    cx.simulate_mouse_down(
        point(px(20.0), px(20.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_down(
        point(px(780.0), px(300.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert_eq!(
        calls.borrow().resizes,
        vec![ResizeEdge::TopLeft, ResizeEdge::Right]
    );

    // Custom left-only button layout + filtered controls + maximized Restore icon.
    view.update(cx, |host, cx| {
        host.state.is_maximized = true;
        host.state.controls = WindowControls {
            fullscreen: false,
            maximize: false,
            minimize: false,
            window_menu: false,
        };
        host.state.button_layout = WindowButtonLayout {
            left: [
                Some(WindowButton::Close),
                Some(WindowButton::Minimize),
                None,
            ],
            right: [Some(WindowButton::Maximize); MAX_BUTTONS_PER_SIDE],
        };
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("csd-controls-left").is_some());
    assert!(cx.debug_bounds("csd-controls-right").is_none());
    assert!(cx.debug_bounds("csd-btn-close").is_some());
    assert!(cx.debug_bounds("csd-btn-minimize").is_none());
    assert!(cx.debug_bounds("csd-btn-maximize").is_none());
    assert_eq!(
        crate::window_chrome::window_button_icon(WindowButton::Maximize, true),
        IconName::WindowRestore
    );
    cx.simulate_mouse_down(
        title_bar.center(),
        MouseButton::Right,
        Modifiers::none(),
    );
    assert_eq!(calls.borrow().menus.len(), 1);

    // Fully tiled mode hides visual outer shadow padding while keeping
    // `client_inset` constant at `CSD_SHADOW_SIZE`.
    view.update(cx, |host, cx| {
        host.state.decorations = Decorations::Client {
            tiling: Tiling::tiled(),
        };
        host.state.controls = WindowControls::default();
        host.state.button_layout =
            crate::window_chrome::default_linux_button_layout();
        cx.notify();
    });
    draw(cx);
    let tiled_bar = cx.debug_bounds("csd-title-bar").expect("tiled bar");
    assert_eq!(tiled_bar.origin, point(px(0.0), px(0.0)));
    assert_eq!(tiled_bar.size.width, px(800.0));
    assert_eq!(
        cx.update(|window, _| window.client_inset()),
        Some(crate::window_chrome::CSD_SHADOW_SIZE)
    );
}

#[gpui_kit::test]
fn injected_csd_confirm_windows_preserve_buttons_and_safe_close(
    cx: &mut TestAppContext,
) {
    use gpui_kit::{
        AppContext as _, Decorations, Modifiers, ParentElement as _, Render,
        Styled as _, Tiling, WindowControls, div, size,
    };
    use ruum_core::confirm::{
        CleanupConfirmDecision, CleanupConfirmItem, CleanupConfirmRequest,
        RemoteConnectConfirmDecision, RemoteConnectConfirmRequest,
    };
    use std::sync::mpsc::channel;

    type CloseFn<V> =
        dyn Fn(&Entity<V>, &mut gpui_kit::Window, &mut gpui_kit::App);

    struct CsdConfirmHost<V: Render + 'static> {
        inner: Entity<V>,
        on_close: std::rc::Rc<CloseFn<V>>,
    }

    impl<V: Render + 'static> Render for CsdConfirmHost<V> {
        fn render(
            &mut self,
            window: &mut gpui_kit::Window,
            _: &mut Context<'_, Self>,
        ) -> impl gpui_kit::IntoElement {
            let state = crate::window_chrome::WindowChromeState {
                decorations: Decorations::Client {
                    tiling: Tiling::default(),
                },
                controls: WindowControls {
                    fullscreen: false,
                    maximize: false,
                    minimize: true,
                    window_menu: true,
                },
                button_layout:
                    crate::window_chrome::default_linux_button_layout(),
                is_maximized: false,
                is_window_active: true,
                window_size: window.bounds().size,
            };
            let entity = self.inner.clone();
            let cb = std::rc::Rc::clone(&self.on_close);
            let actions = crate::window_chrome::WindowChromeActions::new(
                move |window, cx| cb(&entity, window, cx),
            );
            crate::window_chrome::wrap_window_chrome_with(
                "CSD Confirm Test",
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .size_full()
                    .child(self.inner.clone()),
                state,
                actions,
            )
        }
    }

    cx.update(gpui_omarchy::init);
    // Keep a background window alive so closing the confirm window doesn't quit.
    let temp = fixture();
    let (_, cx) = view_over(temp.path(), cx);

    // 1. CleanupConfirmWindow inside CSD at 760x560 and minimum 560x380.
    let req = CleanupConfirmRequest {
        root: temp.path().to_path_buf(),
        title: "CSD Cleanup".into(),
        summary: Some("Verify buttons never clip under CSD".into()),
        items: vec![CleanupConfirmItem {
            path: temp.path().join("junk/blob.bin"),
            bytes: 200_000,
            mode: RemovalMode::Trash,
            reason: Some("cache".into()),
            git_warning: None,
        }],
        docker_items: Vec::new(),
        docker_permission_hint: None,
    };
    let (tx_clean, rx_clean) = channel();
    let (clean_host, clean_cx) = cx.add_window_view(move |_, cx| {
        let inner = cx.new(|cx| {
            crate::CleanupConfirmWindow::new(
                req,
                Some(tx_clean),
                None,
                false,
                cx,
            )
        });
        CsdConfirmHost {
            inner,
            on_close: std::rc::Rc::new(|entity, window, cx| {
                crate::CleanupConfirmWindow::close_from_chrome(
                    &entity.downgrade(),
                    window,
                    cx,
                );
            }),
        }
    });
    clean_cx.simulate_resize(size(px(760.0), px(560.0)));
    draw(clean_cx);
    let cancel_b = clean_cx
        .debug_bounds("confirm-cancel")
        .expect("confirm-cancel visible");
    let clean_b = clean_cx
        .debug_bounds("confirm-clean")
        .expect("confirm-clean visible");
    let bar_b = clean_cx
        .debug_bounds("csd-title-bar")
        .expect("csd-title-bar");
    assert!(cancel_b.top() >= bar_b.bottom());
    assert!(cancel_b.bottom() <= px(560.0));
    assert!(clean_b.bottom() <= px(560.0));

    clean_cx.simulate_resize(size(px(560.0), px(380.0)));
    draw(clean_cx);
    let min_clean_b = clean_cx
        .debug_bounds("confirm-clean")
        .expect("confirm-clean at min size");
    assert!(min_clean_b.bottom() <= px(380.0));

    let close_btn = clean_cx
        .debug_bounds("csd-btn-close")
        .expect("csd close button");
    clean_cx.simulate_click(close_btn.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(rx_clean.try_recv(), Ok(CleanupConfirmDecision::Cancelled));
    drop(clean_host);

    // 2. RemoteConnectConfirmWindow inside CSD at 560x380 and minimum 460x320.
    let remote_req = RemoteConnectConfirmRequest {
        target_label: "staging".into(),
        alias: Some("staging".into()),
        host: "10.0.0.2".into(),
        port: 22,
        username: "ops".into(),
        auth_mode: "password".into(),
        tool_name: "scan_remote_disk".into(),
    };
    let (tx_remote, rx_remote) = channel();
    let (remote_host, remote_cx) = cx.add_window_view(move |_, cx| {
        let inner = cx.new(|cx| {
            crate::RemoteConnectConfirmWindow::new(
                remote_req,
                Some(tx_remote),
                None,
                false,
                cx,
            )
        });
        CsdConfirmHost {
            inner,
            on_close: std::rc::Rc::new(|entity, window, cx| {
                crate::RemoteConnectConfirmWindow::close_from_chrome(
                    &entity.downgrade(),
                    window,
                    cx,
                );
            }),
        }
    });
    remote_cx.simulate_resize(size(px(560.0), px(380.0)));
    draw(remote_cx);
    let deny_b = remote_cx
        .debug_bounds("remote-connect-deny")
        .expect("remote-connect-deny visible");
    let approve_b = remote_cx
        .debug_bounds("remote-connect-approve")
        .expect("remote-connect-approve visible");
    let r_bar_b = remote_cx
        .debug_bounds("csd-title-bar")
        .expect("csd-title-bar");
    assert!(deny_b.top() >= r_bar_b.bottom());
    assert!(deny_b.bottom() <= px(380.0));
    assert!(approve_b.bottom() <= px(380.0));

    remote_cx.simulate_resize(size(px(460.0), px(320.0)));
    draw(remote_cx);
    assert!(remote_cx.debug_bounds("remote-connect-deny").is_some());
    assert!(remote_cx.debug_bounds("remote-connect-approve").is_some());

    let r_close = remote_cx
        .debug_bounds("csd-btn-close")
        .expect("remote csd close");
    remote_cx.simulate_click(r_close.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        rx_remote.try_recv(),
        Ok(RemoteConnectConfirmDecision::Denied)
    );
    drop(remote_host);
}

fn mock_update_result(
    version: &str,
    install_kind: ruum_core::updater::InstallKind,
    can_self_update: bool,
) -> std::sync::Arc<ruum_core::updater::UpdateCheckResult> {
    use std::collections::BTreeMap;

    use ruum_core::storage::UpdateChannel;
    use ruum_core::updater::{Manifest, ManifestAsset, UpdateCheckResult};

    let mut assets = BTreeMap::new();
    let asset = ManifestAsset {
        filename: format!("ruum-{version}-aarch64-macos.zip"),
        path: format!("releases/v{version}/ruum-{version}-aarch64-macos.zip"),
        fallback_url: format!(
            "https://github.com/JsonLee12138/Ruum/releases/download/\
             v{version}/ruum-{version}-aarch64-macos.zip"
        ),
        sha256: "a".repeat(64),
        size: 12_345_678,
    };
    assets.insert("macos-aarch64-app_zip".to_owned(), asset.clone());

    std::sync::Arc::new(UpdateCheckResult {
        current_version: env!("CARGO_PKG_VERSION").to_owned(),
        latest_version: version.to_owned(),
        update_available: true,
        channel: UpdateChannel::Stable,
        install_kind,
        manifest: Some(Manifest {
            schema_version: 1,
            version: version.to_owned(),
            tag: format!("v{version}"),
            channel: "stable".to_owned(),
            published_at: "2026-11-20T00:00:00Z".to_owned(),
            release_url: format!(
                "https://github.com/JsonLee12138/Ruum/releases/tag/v{version}"
            ),
            notes: "### Features\n- Added `ruum update` self-updater\n* \
                    Faster treemap rendering"
                .to_owned(),
            assets,
        }),
        selected_asset_key: Some("macos-aarch64-app_zip".to_owned()),
        selected_asset: Some(asset),
        upgrade_command: match install_kind {
            ruum_core::updater::InstallKind::LinuxSystemPackage => {
                "yay -S ruum-bin".to_owned()
            }
            ruum_core::updater::InstallKind::WindowsPortable => {
                "irm https://ruum-updates.jsonlee12138.com/install.ps1 | iex"
                    .to_owned()
            }
            _ => "curl -fsSL https://ruum-updates.jsonlee12138.com/install.sh | sh"
                .to_owned(),
        },
        can_self_update,
        used_fallback: false,
    })
}

#[gpui_kit::test]
fn settings_updates_tab_toggles_preferences_and_persists_to_isolated_db(
    cx: &mut TestAppContext,
) {
    use ruum_core::storage::{StorageDb, UpdateChannel};
    use ruum_core::updater::InstallKind;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("isolated cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let (view, cx) = view_over(temp.path(), cx);

    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.update_install_kind = InstallKind::MacAppBundle;
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(cx);

    let tab = cx
        .debug_bounds("setting-tab-updates")
        .expect("setting-tab-updates");
    cx.simulate_click(tab.center(), gpui_kit::Modifiers::none());
    draw(cx);

    assert_eq!(
        read(&view, cx, |app| app.settings_category),
        crate::state::SettingsCategory::Updates
    );
    assert!(cx.debug_bounds("setting-updates-section").is_some());
    assert!(cx.debug_bounds("setting-update-check-btn").is_some());
    assert!(cx.debug_bounds("setting-update-auto-check").is_some());
    assert!(cx.debug_bounds("setting-update-channel").is_some());
    assert!(cx.debug_bounds("setting-update-send-client-id").is_some());
    assert!(cx.debug_bounds("setting-update-privacy-note").is_none());

    let auto_check_box = cx
        .debug_bounds("setting-update-auto-check")
        .expect("auto check checkbox");
    cx.simulate_click(auto_check_box.center(), gpui_kit::Modifiers::none());
    draw(cx);

    let send_id_box = cx
        .debug_bounds("setting-update-send-client-id")
        .expect("send client id checkbox");
    cx.simulate_click(send_id_box.center(), gpui_kit::Modifiers::none());
    draw(cx);

    update(&view, cx, |app, cx| {
        app.set_update_channel(UpdateChannel::Beta, cx);
    });
    draw(cx);

    let db = StorageDb::open(&db_path).expect("reopen isolated db");
    let persisted = db.load_update_settings().expect("load update settings");
    assert!(!persisted.auto_check);
    assert!(!persisted.send_client_id);
    assert_eq!(persisted.channel, UpdateChannel::Beta);
}

#[gpui_kit::test]
fn icon_rail_update_entry_tracks_active_states_and_opens_modal(
    cx: &mut TestAppContext,
) {
    use crate::state::UpdateUiState;
    use ruum_core::updater::InstallKind;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    draw(cx);

    // Hidden when Idle / Checking / UpToDate / Error.
    assert!(cx.debug_bounds("rail-update").is_none());

    for hidden_state in [
        UpdateUiState::Checking,
        UpdateUiState::UpToDate {
            checked_at_secs: 1_700_000_000,
        },
        UpdateUiState::Error {
            message: "network error".to_owned(),
            info: None,
        },
    ] {
        update(&view, cx, |app, cx| {
            app.update_state = hidden_state;
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("rail-update").is_none());
    }

    // Visible when Available (unskipped), Downloading, ReadyToInstall, Installing.
    let info = mock_update_result("0.9.0", InstallKind::MacAppBundle, true);
    for visible_state in [
        UpdateUiState::Available {
            info: std::sync::Arc::clone(&info),
        },
        UpdateUiState::Downloading {
            info: std::sync::Arc::clone(&info),
            downloaded: 500,
            total: 1000,
        },
        UpdateUiState::ReadyToInstall {
            info: std::sync::Arc::clone(&info),
            artifact_path: temp.path().join("ruum.zip"),
        },
        UpdateUiState::Installing {
            info: std::sync::Arc::clone(&info),
        },
    ] {
        update(&view, cx, |app, cx| {
            app.update_state = visible_state;
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("rail-update").is_some());
        assert!(cx.debug_bounds("rail-update-dot").is_some());
    }

    // Clicking rail-update opens update-modal.
    let btn = cx.debug_bounds("rail-update").expect("rail-update");
    cx.simulate_click(btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(read(&view, cx, |app| app.update_modal_open));
    assert!(cx.debug_bounds("update-modal").is_some());
}

#[gpui_kit::test]
fn update_banner_shows_once_per_version_is_non_blocking_and_respects_guards(
    cx: &mut TestAppContext,
) {
    use crate::state::UpdateUiState;
    use ruum_core::confirm::RemoteConnectConfirmRequest;
    use ruum_core::updater::InstallKind;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    let info_v1 = mock_update_result("0.9.0", InstallKind::MacAppBundle, true);

    update(&view, cx, |app, cx| {
        app.update_state = UpdateUiState::Available {
            info: std::sync::Arc::clone(&info_v1),
        };
        cx.notify();
    });
    draw(cx);

    // Shown on Explore and does NOT steal keyboard focus from `app.focus`.
    assert!(cx.debug_bounds("update-banner").is_some());
    assert!(cx.debug_bounds("update-banner-view").is_some());
    assert!(cx.debug_bounds("update-banner-close").is_some());
    let root_focused = cx.update(|window, cx| {
        view.read_with(cx, |app, _| app.focus.is_focused(window))
    });
    assert!(root_focused, "update banner must not steal keyboard focus");

    // Suppressed when Review, Running, or modals are active.
    update(&view, cx, |app, cx| {
        app.screen = Screen::Review;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-banner").is_none());

    update(&view, cx, |app, cx| {
        app.screen = Screen::Running;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-banner").is_none());

    update(&view, cx, |app, cx| {
        app.screen = Screen::Explore;
        app.confirm_open = true;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-banner").is_none());

    update(&view, cx, |app, cx| {
        app.confirm_open = false;
        app.volumes_open = true;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-banner").is_none());

    update(&view, cx, |app, cx| {
        app.volumes_open = false;
        app.remote_docker_modal_open = true;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-banner").is_none());

    update(&view, cx, |app, cx| {
        app.remote_docker_modal_open = false;
        app.enqueue_remote_connect_confirm(
            RemoteConnectConfirmRequest {
                target_label: "prod".to_owned(),
                alias: Some("prod".to_owned()),
                host: "10.0.0.2".to_owned(),
                port: 22,
                username: "deploy".to_owned(),
                auth_mode: "private_key".to_owned(),
                tool_name: "scan_directory".to_owned(),
            },
            None,
            None,
            cx,
        );
    });
    draw(cx);
    assert!(cx.debug_bounds("update-banner").is_none());

    update(&view, cx, |app, cx| {
        app.deny_remote_connect(cx);
        app.generated_script_dialog =
            Some(crate::state::GeneratedScriptDialog {
                profile_id: "p1".to_owned(),
                alias: "prod".to_owned(),
                script_path: temp.path().join("clean.sh"),
                item_count: 1,
                reclaim_bytes: 1024,
            });
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-banner").is_none());

    // Clear suppression and verify clicking `x` dismisses for 0.9.0 only.
    update(&view, cx, |app, cx| {
        app.generated_script_dialog = None;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-banner").is_some());

    let close_btn = cx
        .debug_bounds("update-banner-close")
        .expect("update-banner-close");
    cx.simulate_click(close_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(cx.debug_bounds("update-banner").is_none());
    assert_eq!(
        read(&view, cx, |app| {
            app.update_banner_dismissed_version.clone()
        }),
        Some("0.9.0".to_owned())
    );

    // Newer version 0.9.1 shows banner again; clicking View opens modal.
    let info_v2 = mock_update_result("0.9.1", InstallKind::MacAppBundle, true);
    update(&view, cx, |app, cx| {
        app.update_state = UpdateUiState::Available { info: info_v2 };
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-banner").is_some());

    let view_btn = cx
        .debug_bounds("update-banner-view")
        .expect("update-banner-view");
    cx.simulate_click(view_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(read(&view, cx, |app| app.update_modal_open));
    assert!(cx.debug_bounds("update-modal").is_some());
    assert!(cx.debug_bounds("update-banner").is_none());
}

#[gpui_kit::test]
fn update_modal_renders_all_states_hideable_download_marks_warning_and_skip(
    cx: &mut TestAppContext,
) {
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::state::UpdateUiState;
    use ruum_core::removal::Target;
    use ruum_core::storage::StorageDb;
    use ruum_core::updater::InstallKind;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("isolated cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let (view, cx) = view_over(temp.path(), cx);

    let self_info =
        mock_update_result("0.9.5", InstallKind::MacAppBundle, true);
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.update_install_kind = InstallKind::MacAppBundle;
        app.update_state = UpdateUiState::Available {
            info: std::sync::Arc::clone(&self_info),
        };
        app.open_update_modal(cx);
    });
    draw(cx);

    assert!(cx.debug_bounds("update-modal").is_some());
    assert!(cx.debug_bounds("update-changelog-scroll").is_some());
    assert!(cx.debug_bounds("update-changelog-heading-0").is_some());
    assert!(cx.debug_bounds("update-changelog-item-1").is_some());
    assert!(cx.debug_bounds("update-download-btn").is_some());
    assert!(cx.debug_bounds("update-later-btn").is_some());
    assert!(cx.debug_bounds("update-skip-btn").is_some());

    // Guide-only install kind renders Copy Command + Open Release instead of
    // Download & Update.
    let pkg_info =
        mock_update_result("0.9.5", InstallKind::LinuxSystemPackage, false);
    update(&view, cx, |app, cx| {
        app.update_install_kind = InstallKind::LinuxSystemPackage;
        app.update_state = UpdateUiState::Available { info: pkg_info };
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-download-btn").is_none());
    assert!(cx.debug_bounds("update-copy-cmd-btn").is_some());
    assert!(cx.debug_bounds("update-open-release-btn").is_some());

    // Downloading state: progress bar + Hide keeps download running in bg.
    let cancel_flag = std::sync::Arc::new(AtomicBool::new(false));
    update(&view, cx, |app, cx| {
        app.update_install_kind = InstallKind::MacAppBundle;
        app.update_download_cancel = Some(std::sync::Arc::clone(&cancel_flag));
        app.update_state = UpdateUiState::Downloading {
            info: std::sync::Arc::clone(&self_info),
            downloaded: 4_000_000,
            total: 10_000_000,
        };
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-progress-bar").is_some());
    assert!(cx.debug_bounds("update-hide-btn").is_some());
    assert!(cx.debug_bounds("update-cancel-btn").is_some());

    let hide_btn = cx.debug_bounds("update-hide-btn").expect("hide btn");
    cx.simulate_click(hide_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(!read(&view, cx, |app| app.update_modal_open));
    assert!(!cancel_flag.load(Ordering::Relaxed));
    assert!(matches!(
        read(&view, cx, |app| app.update_state.clone()),
        UpdateUiState::Downloading { .. }
    ));
    assert!(cx.debug_bounds("rail-update").is_some());

    // Reopen and cancel download -> reverts to Available and flips flag.
    update(&view, cx, Ruum::open_update_modal);
    draw(cx);
    let cancel_btn = cx.debug_bounds("update-cancel-btn").expect("cancel btn");
    cx.simulate_click(cancel_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    assert!(cancel_flag.load(Ordering::Relaxed));
    assert!(matches!(
        read(&view, cx, |app| app.update_state.clone()),
        UpdateUiState::Available { .. }
    ));

    // ReadyToInstall with active marks renders restart warning.
    update(&view, cx, |app, cx| {
        app.marks.toggle(Target {
            path: temp.path().join("junk"),
            bytes: 300_000,
            is_dir: true,
            hidden: false,
        });
        app.update_state = UpdateUiState::ReadyToInstall {
            info: std::sync::Arc::clone(&self_info),
            artifact_path: temp.path().join("ruum.zip"),
        };
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-restart-install-btn").is_some());
    assert!(cx.debug_bounds("update-restart-warning").is_some());

    // Error state shows error box + Retry + Open GitHub Release.
    update(&view, cx, |app, cx| {
        app.update_state = UpdateUiState::Error {
            message: "SHA-256 mismatch".to_owned(),
            info: Some(std::sync::Arc::clone(&self_info)),
        };
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("update-error-box").is_some());
    assert!(cx.debug_bounds("update-retry-btn").is_some());
    assert!(cx.debug_bounds("update-open-release-btn").is_some());

    // Skip version persists to isolated DB and hides rail-update & banner.
    update(&view, cx, |app, cx| {
        app.update_state = UpdateUiState::Available {
            info: std::sync::Arc::clone(&self_info),
        };
        cx.notify();
    });
    draw(cx);
    let skip_btn = cx.debug_bounds("update-skip-btn").expect("skip btn");
    cx.simulate_click(skip_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);

    assert!(!read(&view, cx, |app| app.update_modal_open));
    assert!(cx.debug_bounds("rail-update").is_none());
    assert!(cx.debug_bounds("update-banner").is_none());
    let db = StorageDb::open(&db_path).expect("reopen db");
    let saved = db.load_update_settings().expect("load saved");
    assert_eq!(saved.skipped_version.as_deref(), Some("0.9.5"));
}

#[gpui_kit::test]
fn update_ui_renders_cleanly_across_all_four_locales_and_runtime_reopen(
    cx: &mut TestAppContext,
) {
    use crate::i18n::Locale;
    use crate::state::UpdateUiState;
    use ruum_core::updater::InstallKind;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let info = mock_update_result("0.9.9", InstallKind::MacAppBundle, true);

    cx.update(|cx| {
        crate::install_notification_handler(cx);
        crate::install_test_runtime(temp.path().to_path_buf(), false, None, cx);
        crate::with_runtime_mut(cx, |rt| {
            rt.update_install_kind = InstallKind::MacAppBundle;
            rt.update_state = UpdateUiState::Downloading {
                info: std::sync::Arc::clone(&info),
                downloaded: 256_000,
                total: 1_024_000,
            };
        });
        crate::open_or_navigate_main_window(None, cx);
    });
    cx.run_until_parked();

    let win1 = cx
        .read(|cx| cx.global::<crate::AppRuntime>().main_window)
        .expect("win1");
    win1.update(cx, |app, _, cx| {
        assert!(matches!(
            app.update_state,
            UpdateUiState::Downloading {
                downloaded: 256_000,
                total: 1_024_000,
                ..
            }
        ));
        for locale in [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko] {
            app.set_locale(locale);
            app.navigate_rail(Screen::Settings, cx);
            app.set_settings_category(
                crate::state::SettingsCategory::Updates,
                cx,
            );
            app.open_update_modal(cx);
            cx.notify();
        }
    })
    .expect("win1 update");
    cx.run_until_parked();

    // Close main window to tray and reopen: update progress persists!
    win1.update(cx, |_, window, cx| {
        window.remove_window();
        crate::with_runtime_mut(cx, |rt| rt.main_window = None);
    })
    .expect("close win1");
    cx.run_until_parked();

    cx.update(|cx| {
        crate::open_or_navigate_main_window(None, cx);
    });
    cx.run_until_parked();

    let win2 = cx
        .read(|cx| cx.global::<crate::AppRuntime>().main_window)
        .expect("win2");
    win2.update(cx, |app, _, _| {
        assert!(matches!(
            app.update_state,
            UpdateUiState::Downloading {
                downloaded: 256_000,
                total: 1_024_000,
                ..
            }
        ));
    })
    .expect("win2 update");
}

#[gpui_kit::test]
fn update_modal_handles_none_manifest_and_friendly_errors_across_locales(
    cx: &mut TestAppContext,
) {
    use crate::i18n::Locale;
    use crate::state::UpdateUiState;
    use ruum_core::storage::UpdateSettings;
    use ruum_core::updater::{InstallKind, UpdateCheckResult};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);

    let base = mock_update_result("0.9.8", InstallKind::MacAppBundle, true);
    let no_manifest = std::sync::Arc::new(UpdateCheckResult {
        manifest: None,
        ..(*base).clone()
    });

    // Empty channel / no-release check result transitions cleanly to UpToDate.
    let no_release = UpdateCheckResult {
        update_available: false,
        manifest: None,
        selected_asset_key: None,
        selected_asset: None,
        ..(*base).clone()
    };
    update(&view, cx, |app, cx| {
        app.apply_update_check_outcome(
            Ok((UpdateSettings::default(), no_release)),
            cx,
        );
    });
    draw(cx);
    assert!(matches!(
        read(&view, cx, |app| app.update_state.clone()),
        UpdateUiState::UpToDate { .. }
    ));
    assert!(read(&view, cx, |app| app.update_last_error.clone()).is_none());

    // Available with manifest = None skips summary/changelog and falls back to
    // the repository releases URL.
    update(&view, cx, |app, cx| {
        app.update_state = UpdateUiState::Available {
            info: std::sync::Arc::clone(&no_manifest),
        };
        app.open_update_modal(cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("update-modal").is_some());
    assert!(cx.debug_bounds("update-release-summary-card").is_none());
    assert!(cx.debug_bounds("update-changelog-scroll").is_none());
    assert_eq!(
        read(&view, cx, crate::state::Ruum::resolved_update_release_url),
        format!(
            "https://github.com/{}/releases",
            ruum_core::updater::DEFAULT_GITHUB_REPO
        )
    );

    // ReadyToInstall with manifest = None guards apply_ready_update cleanly.
    update(&view, cx, |app, cx| {
        app.update_state = UpdateUiState::ReadyToInstall {
            info: std::sync::Arc::clone(&no_manifest),
            artifact_path: temp.path().join("ruum.zip"),
        };
        app.apply_ready_update(cx);
    });
    draw(cx);
    assert!(matches!(
        read(&view, cx, |app| app.update_state.clone()),
        UpdateUiState::Error { .. }
    ));
    assert!(cx.debug_bounds("update-error-box").is_some());

    // Error state renders across all 4 locales in both Settings and Modal
    // while keeping raw diagnostic in app.update_last_error.
    update(&view, cx, |app, cx| {
        app.apply_update_check_outcome(
            Err("worker returned HTTP 503".to_owned()),
            cx,
        );
        app.navigate_rail(Screen::Settings, cx);
        app.set_settings_category(crate::state::SettingsCategory::Updates, cx);
    });
    assert_eq!(
        read(&view, cx, |app| app.update_last_error.clone()).as_deref(),
        Some("worker returned HTTP 503")
    );

    for locale in [Locale::En, Locale::Zh, Locale::Ja, Locale::Ko] {
        update(&view, cx, |app, cx| {
            app.set_locale(locale);
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("setting-updates-section").is_some());
        assert!(cx.debug_bounds("setting-update-privacy-note").is_none());
        assert!(cx.debug_bounds("update-error-box").is_some());
    }
}

fn read_test_http_request(
    stream: &mut std::net::TcpStream,
) -> (String, String) {
    use std::io::Read as _;

    let mut buf = Vec::new();
    let mut chunk = [0_u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut chunk).expect("read mock http");
        assert!(n > 0, "unexpected EOF before headers");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let content_len = headers
        .lines()
        .find_map(|line| {
            let (name, val) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                val.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    let total_len = header_end + 4 + content_len;
    while buf.len() < total_len {
        let n = stream.read(&mut chunk).expect("read mock http body");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body =
        String::from_utf8_lossy(&buf[header_end + 4..total_len]).into_owned();
    (headers, body)
}

fn write_test_http_response(
    stream: &mut std::net::TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) {
    use std::io::Write as _;

    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).expect("write mock http");
    stream.flush().expect("flush mock http");
}

#[gpui_kit::test]
fn settings_api_provider_toggles_fields_links_default_urls_and_preserves_custom(
    cx: &mut TestAppContext,
) {
    use ruum_core::watchdog_config::{AiCliProvider, ApiProtocol};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1000.), px(1400.)));

    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Settings, cx);
        app.set_settings_category(crate::state::SettingsCategory::Ai, cx);
    });
    draw(cx);

    // Initially Agy: CLI extra args visible, API controls hidden, API button present.
    assert!(cx.debug_bounds("provider-api").is_some());
    assert!(cx.debug_bounds("setting-extra-args-input").is_some());
    assert!(cx.debug_bounds("setting-api-protocol-openai").is_none());
    assert!(cx.debug_bounds("setting-api-base-url-input").is_none());
    assert!(cx.debug_bounds("setting-api-key-input").is_none());

    // Pre-populate custom command and extra args under Custom, then switch to Api.
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.set_watchdog_provider(AiCliProvider::Custom, cx);
            app.ai_config.custom_command = "/opt/bin/old-cli".into();
            app.watchdog_input_custom_cmd.as_ref().unwrap().update(
                cx,
                |i, cx| {
                    i.set_value("/opt/bin/old-cli", window, cx);
                },
            );
            app.watchdog_input_extra_args
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| i.set_value("--json", window, cx));
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("setting-custom-cmd-input").is_some());
    assert!(cx.debug_bounds("setting-extra-args-input").is_some());

    // Click the API provider button.
    let api_btn = cx.debug_bounds("provider-api").expect("provider-api");
    cx.simulate_click(api_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);

    read(&view, cx, |app| {
        assert_eq!(app.watchdog_draft.ai_provider, AiCliProvider::Api);
        assert_eq!(app.watchdog_draft.api_protocol, ApiProtocol::OpenAi);
    });
    // CLI inputs MUST be hidden when Provider::Api is active.
    assert!(cx.debug_bounds("setting-custom-cmd-input").is_none());
    assert!(cx.debug_bounds("setting-extra-args-input").is_none());
    // API inputs MUST be visible, and API key input MUST be masked.
    assert!(cx.debug_bounds("setting-api-protocol-openai").is_some());
    assert!(cx.debug_bounds("setting-api-protocol-anthropic").is_some());
    assert!(cx.debug_bounds("setting-api-base-url-input").is_some());
    assert!(cx.debug_bounds("setting-api-key-input").is_some());
    assert!(cx.debug_bounds("setting-clear-api-key").is_some());
    assert!(cx.debug_bounds("setting-model-input").is_some());
    assert!(cx.debug_bounds("setting-test-cli").is_some());
    assert!(cx.debug_bounds("setting-fetch-models").is_some());

    let api_key_input =
        read(&view, cx, |app| app.watchdog_input_api_key.clone())
            .expect("api key input");
    assert!(
        cx.update(|_, cx| api_key_input.read(cx).presentation().is_masked())
    );

    let base_url_input =
        read(&view, cx, |app| app.watchdog_input_api_base_url.clone())
            .expect("base url input");
    assert_eq!(
        cx.update(|_, cx| {
            base_url_input
                .read(cx)
                .presentation()
                .placeholder()
                .to_string()
        }),
        ApiProtocol::OpenAi.default_base_url()
    );

    // Switching OpenAI -> Claude when Base URL is empty updates placeholder & value.
    let claude_btn = cx
        .debug_bounds("setting-api-protocol-anthropic")
        .expect("claude button");
    cx.simulate_click(claude_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    read(&view, cx, |app| {
        assert_eq!(app.watchdog_draft.api_protocol, ApiProtocol::Anthropic);
    });
    assert_eq!(
        cx.update(|_, cx| base_url_input.read(cx).value().to_string()),
        ApiProtocol::Anthropic.default_base_url()
    );
    assert_eq!(
        cx.update(|_, cx| {
            base_url_input
                .read(cx)
                .presentation()
                .placeholder()
                .to_string()
        }),
        ApiProtocol::Anthropic.default_base_url()
    );

    // Switching Claude -> OpenAI when Base URL equals Claude default updates to OpenAI default.
    let openai_btn = cx
        .debug_bounds("setting-api-protocol-openai")
        .expect("openai button");
    cx.simulate_click(openai_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    read(&view, cx, |app| {
        assert_eq!(app.watchdog_draft.api_protocol, ApiProtocol::OpenAi);
    });
    assert_eq!(
        cx.update(|_, cx| base_url_input.read(cx).value().to_string()),
        ApiProtocol::OpenAi.default_base_url()
    );
    assert_eq!(
        cx.update(|_, cx| {
            base_url_input
                .read(cx)
                .presentation()
                .placeholder()
                .to_string()
        }),
        ApiProtocol::OpenAi.default_base_url()
    );

    // Entering a custom Base URL (e.g. MiMo) MUST NOT be overwritten when toggling protocol.
    cx.update(|window, cx| {
        base_url_input.update(cx, |i, cx| {
            i.set_value("https://api.xiaomimimo.com/v1", window, cx);
        });
    });
    let claude_btn = cx
        .debug_bounds("setting-api-protocol-anthropic")
        .expect("claude button");
    cx.simulate_click(claude_btn.center(), gpui_kit::Modifiers::none());
    draw(cx);
    read(&view, cx, |app| {
        assert_eq!(app.watchdog_draft.api_protocol, ApiProtocol::Anthropic);
    });
    assert_eq!(
        cx.update(|_, cx| base_url_input.read(cx).value().to_string()),
        "https://api.xiaomimimo.com/v1"
    );
    assert_eq!(
        cx.update(|_, cx| {
            base_url_input
                .read(cx)
                .presentation()
                .placeholder()
                .to_string()
        }),
        ApiProtocol::Anthropic.default_base_url()
    );
}

#[gpui_kit::test]
fn settings_api_key_lazy_keychain_save_preserve_clear_and_draft_probe(
    cx: &mut TestAppContext,
) {
    use ruum_core::storage::{AuthMode, SshProfile, StorageDb};
    use ruum_core::watchdog_config::{AiCliProvider, ApiProtocol};
    use std::net::TcpListener;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let enc_path = cfg_dir.path().join("device-credentials.enc");

    // Pre-seed an SSH profile + encrypted credential file so has_encrypted_file() == true.
    let counter_seed = CountingRootSecret::new([0x33; 32]);
    {
        let db = StorageDb::open(&db_path).expect("open db");
        db.add_ssh_profile(&SshProfile {
            id: "prof-ssh".into(),
            alias: "prod".into(),
            host: "10.0.0.9".into(),
            port: 22,
            username: "ops".into(),
            auth_mode: AuthMode::PrivateKey,
            default_path: None,
            created_at_secs: 100,
            updated_at_secs: 100,
            last_total_bytes: None,
            last_available_bytes: None,
            last_scanned_secs: None,
            pending_script_path: None,
            pending_script_items: None,
            pending_reclaim_bytes: None,
            pending_created_secs: None,
        })
        .expect("seed ssh");
        let seed_store = counter_seed.store_for(&enc_path);
        seed_store
            .set_ssh_secret(
                "prof-ssh",
                &ruum_core::credentials::SshCredentialSecret::Password {
                    password: "pw".into(),
                },
            )
            .expect("seed enc file");
    }

    // Fresh counter starting at 0 for the GUI lifecycle.
    let counter = CountingRootSecret::new([0x33; 32]);
    let store = counter.store_for(&enc_path);
    assert!(store.has_encrypted_file());
    assert_eq!(counter.load_count(), 0);

    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1000.), px(1400.)));

    // 1. Navigate to Settings -> AI, switch to API, draw, and save with EMPTY key:
    //    keychain MUST stay at 0 unlocks!
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.db_path = Some(db_path.clone());
            app.cred_store = Some(store.clone());
            app.reload_watchdog_config_and_prepare_settings();
            app.navigate_rail(Screen::Settings, cx);
            app.set_settings_category(crate::state::SettingsCategory::Ai, cx);
            app.set_watchdog_provider(AiCliProvider::Api, cx);
            app.save_watchdog_settings(window, cx);
        });
    });
    draw(cx);
    assert_eq!(
        counter.load_count(),
        0,
        "startup, Settings render, Provider::Api switch, and empty-key save must never unlock keychain"
    );

    // 2. Local TcpListener mock for OpenAI-compatible GET /models + POST /chat/completions.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock api");
    let addr = listener.local_addr().expect("mock addr");
    let server = std::thread::spawn(move || {
        for idx in 0..4 {
            let (mut stream, _) = listener.accept().expect("accept");
            let (headers, _body) = read_test_http_request(&mut stream);
            let headers_lc = headers.to_ascii_lowercase();
            assert!(
                headers_lc.contains("authorization: bearer test-api-key"),
                "request #{idx} missing draft/saved bearer key: {headers}"
            );
            match idx {
                0 | 1 | 3 => {
                    write_test_http_response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        r#"{"data":[{"id":"mimo-v2-flash"}]}"#,
                    );
                }
                _ => {
                    let sse = concat!(
                        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",",
                        "\"model\":\"mimo-v2-flash\",\"choices\":[{\"index\":0,",
                        "\"delta\":{\"role\":\"assistant\",\"content\":\"OK\"},",
                        "\"finish_reason\":null}]}\n\n",
                        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",",
                        "\"model\":\"mimo-v2-flash\",\"choices\":[{\"index\":0,",
                        "\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                        "data: [DONE]\n\n"
                    );
                    write_test_http_response(
                        &mut stream,
                        "200 OK",
                        "text/event-stream",
                        sse,
                    );
                }
            }
        }
    });

    // 3. Enter draft Base URL, Model, and API Key WITHOUT clicking Save yet,
    //    then test connection, fetch models, and benchmark speed immediately.
    let base_url = format!("http://{addr}/v1");
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.watchdog_input_api_base_url.as_ref().unwrap().update(
                cx,
                |i, cx| {
                    i.set_value(base_url.clone(), window, cx);
                },
            );
            app.watchdog_input_model
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| {
                    i.set_value("mimo-v2-flash", window, cx);
                });
            app.watchdog_input_api_key
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| {
                    i.set_value("test-api-key", window, cx);
                });
        });
    });

    let test_btn = cx.debug_bounds("setting-test-cli").expect("test btn");
    cx.simulate_click(test_btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    read(&view, cx, |app| {
        assert!(!app.watchdog_testing);
        let (msg, status) = app.notice.as_ref().expect("test notice");
        assert_eq!(*status, gpui_omarchy::Status::Success, "{msg}");
    });

    let fetch_btn = cx.debug_bounds("setting-fetch-models").expect("fetch btn");
    cx.simulate_click(fetch_btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    read(&view, cx, |app| {
        assert!(!app.watchdog_fetching_models);
        assert_eq!(app.watchdog_models.len(), 1);
        assert_eq!(app.watchdog_models[0].id, "mimo-v2-flash");
    });

    draw(cx);
    let speed_btn = cx
        .debug_bounds("setting-model-speed-0")
        .expect("speed btn 0");
    cx.simulate_click(speed_btn.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    read(&view, cx, |app| {
        assert!(app.watchdog_benchmarking_models.is_empty());
        assert_eq!(app.watchdog_speed_results.len(), 1);
        assert!(app.watchdog_speed_results[0].ok);
        assert_eq!(app.watchdog_speed_results[0].model, "mimo-v2-flash");
    });
    // Draft operations used the draft key directly with 0 keychain unlocks!
    assert_eq!(counter.load_count(), 0);

    // 4. Save Settings: persists non-secret config to SQLite and API key to LazyCredentialStore.
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.save_watchdog_settings(window, cx);
        });
    });
    assert_eq!(counter.load_count(), 1);
    let api_key_input = read(&view, cx, |app| {
        assert_eq!(app.ai_config.provider, AiCliProvider::Api);
        assert_eq!(app.ai_config.api_protocol, ApiProtocol::OpenAi);
        assert_eq!(app.ai_config.api_base_url, base_url);
        assert_eq!(app.ai_config.model.as_deref(), Some("mimo-v2-flash"));
        assert_eq!(app.api_key_session_known, Some(true));
        app.watchdog_input_api_key.clone().expect("api key input")
    });
    let key_val = cx.update(|_, cx| api_key_input.read(cx).value().to_string());
    assert!(key_val.is_empty(), "key input should clear after saving");
    let db = StorageDb::open(&db_path).expect("reopen db");
    let saved_ai = db.load_ai_config().expect("load saved ai");
    assert_eq!(saved_ai.provider, AiCliProvider::Api);
    assert_eq!(saved_ai.api_protocol, ApiProtocol::OpenAi);
    assert_eq!(saved_ai.api_base_url, base_url);

    // 5. Fresh Ruum instance with a fresh 0-count store: rendering Settings does
    //    NOT unlock keychain, saving empty key preserves existing key, and
    //    testing connection with empty input lazily reads the saved key.
    let fresh_counter = CountingRootSecret::new([0x33; 32]);
    let fresh_store = fresh_counter.store_for(&enc_path);
    let (fresh_view, fresh_cx) = view_over(temp.path(), &mut cx.cx);
    fresh_cx.simulate_resize(gpui_kit::size(px(1000.), px(1400.)));

    fresh_cx.update(|window, cx| {
        fresh_view.update(cx, |app, cx| {
            app.db_path = Some(db_path.clone());
            app.cred_store = Some(fresh_store.clone());
            app.reload_watchdog_config_and_prepare_settings();
            app.navigate_rail(Screen::Settings, cx);
            app.set_settings_category(crate::state::SettingsCategory::Ai, cx);
            app.ensure_watchdog_inputs(window, cx);
            app.watchdog_input_model
                .as_ref()
                .unwrap()
                .update(cx, |i, cx| {
                    i.set_value("deepseek-chat", window, cx);
                });
            app.save_watchdog_settings(window, cx);
        });
    });
    draw(fresh_cx);
    assert_eq!(
        fresh_counter.load_count(),
        0,
        "saving with empty API key must preserve existing key without unlocking keychain"
    );

    let test_btn = fresh_cx
        .debug_bounds("setting-test-cli")
        .expect("fresh test btn");
    fresh_cx.simulate_click(test_btn.center(), gpui_kit::Modifiers::none());
    fresh_cx.run_until_parked();
    assert_eq!(fresh_counter.load_count(), 1);
    read(&fresh_view, fresh_cx, |app| {
        let (msg, status) = app.notice.as_ref().expect("fresh test notice");
        assert_eq!(*status, gpui_omarchy::Status::Success, "{msg}");
    });
    server.join().expect("mock api server join");

    // 6. Clicking Clear Key removes the stored API key cleanly.
    let clear_btn = fresh_cx
        .debug_bounds("setting-clear-api-key")
        .expect("clear api key btn");
    fresh_cx.simulate_click(clear_btn.center(), gpui_kit::Modifiers::none());
    draw(fresh_cx);
    read(&fresh_view, fresh_cx, |app| {
        assert_eq!(app.api_key_session_known, Some(false));
    });
    assert_eq!(
        fresh_store.get_ai_api_key().expect("read after clear"),
        None
    );
}

#[gpui_kit::test]
fn settings_api_renders_cleanly_across_all_four_locales(
    cx: &mut TestAppContext,
) {
    use crate::i18n::ALL_LOCALES;
    use ruum_core::watchdog_config::AiCliProvider;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1000.), px(1400.)));

    update(&view, cx, |app, cx| {
        app.navigate_rail(Screen::Settings, cx);
        app.set_settings_category(crate::state::SettingsCategory::Ai, cx);
        app.set_watchdog_provider(AiCliProvider::Api, cx);
    });

    for locale in ALL_LOCALES {
        update(&view, cx, |app, cx| {
            app.set_locale(locale);
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("provider-api").is_some());
        assert!(cx.debug_bounds("setting-api-protocol-openai").is_some());
        assert!(cx.debug_bounds("setting-api-protocol-anthropic").is_some());
        assert!(cx.debug_bounds("setting-api-base-url-input").is_some());
        assert!(cx.debug_bounds("setting-api-key-input").is_some());
        assert!(cx.debug_bounds("setting-clear-api-key").is_some());
        assert!(cx.debug_bounds("setting-custom-cmd-input").is_none());
        assert!(cx.debug_bounds("setting-extra-args-input").is_none());
    }
}

#[gpui_kit::test]
fn local_ai_analysis_failure_notifies_and_persists_open_and_closed(
    cx: &mut TestAppContext,
) {
    use crate::i18n::Locale;
    use ruum_core::removal::RemovalMode;
    use ruum_core::storage::{AiState, StorageDb};
    use ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS;
    use ruum_core::watchdog_config::{
        AiCliProvider, CleanupProposal, ProposalItem,
    };
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    cx.update(gpui_omarchy::init);
    cx.update(|cx| crate::install_notification_handler(cx));
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let enc_path = cfg_dir.path().join("creds.enc");

    let prior = CleanupProposal {
        timestamp_secs: 100,
        summary: "Existing local proposal".into(),
        items: vec![ProposalItem::new(
            temp.path().join("junk/blob.bin"),
            1024,
            RemovalMode::Trash,
            None,
            None,
        )],
        docker_targets: Vec::new(),
    };
    {
        let db = StorageDb::open(&db_path).expect("open db");
        let st = AiState::from_proposal(&prior, 100).expect("state");
        db.save_ai_state(&st).expect("save prior");
    }

    let counter = CountingRootSecret::new([0x51; 32]);
    let store = counter.store_for(&enc_path);
    store.set_ai_api_key("test-key").expect("save api key");

    // 1. Open main window: timeout error shows Status::Error toast, emits
    //    NOTIF_TAG_AI_ANALYSIS, preserves existing proposal, and renders on
    //    Watchdog page.
    let listener_fail =
        TcpListener::bind("127.0.0.1:0").expect("bind fail api");
    let addr_fail = listener_fail.local_addr().expect("addr fail");
    let stop_fail = Arc::new(AtomicBool::new(false));
    let stop_fail_srv = Arc::clone(&stop_fail);
    let server_fail = std::thread::spawn(move || {
        while !stop_fail_srv.load(Ordering::Acquire) {
            let Ok((mut stream, _)) = listener_fail.accept() else {
                break;
            };
            if stop_fail_srv.load(Ordering::Acquire) {
                break;
            }
            let _ = read_test_http_request(&mut stream);
            write_test_http_response(
                &mut stream,
                "504 Gateway Timeout",
                "application/json",
                r#"{"error":{"message":"upstream request timed out after 30s"}}"#,
            );
        }
    });

    let (view, win_cx) = view_over(temp.path(), cx);
    update(&view, win_cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.cred_store = Some(store.clone());
        app.set_locale(Locale::Zh);
        app.ai_config.provider = AiCliProvider::Api;
        app.watchdog_draft.ai_provider = AiCliProvider::Api;
        app.ai_config.api_base_url = format!("http://{addr_fail}/v1");
        app.ai_config.model = Some("gpt-test".into());
        app.watchdog_draft.model = Some("gpt-test".into());
        app.watchdog_input_api_base_url = None;
        app.watchdog_input_model = None;
        app.reload_watchdog_state();
        app.navigate_rail(Screen::Watchdog, cx);
        app.run_ai_analysis_now(cx);
    });
    win_cx.run_until_parked();
    stop_fail.store(true, Ordering::Release);
    let _ = TcpStream::connect(addr_fail);
    server_fail.join().expect("server fail join");

    read(&view, win_cx, |app| {
        assert!(!app.watchdog_analyzing);
        assert!(matches!(app.notice, Some((_, gpui_omarchy::Status::Error))));
        assert!(app.ai_last_error.is_some());
        assert!(app.ai_proposal.is_some());
    });
    draw(win_cx);
    assert!(win_cx.debug_bounds("watchdog-ai-last-error").is_some());

    let shown = win_cx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].tag.as_ref(), NOTIF_TAG_AI_ANALYSIS);
    assert_eq!(
        shown[0].title.as_ref(),
        crate::i18n::messages::ai_analysis_timed_out_notif_title(Locale::Zh)
    );
    assert!(shown[0].body.chars().count() <= 160);

    let saved = StorageDb::open(&db_path)
        .expect("reopen db")
        .load_ai_state()
        .expect("load ai state");
    assert_eq!(saved.last_error.as_deref(), Some(shown[0].body.as_ref()));
    assert!(saved.last_error_at_secs.is_some());
    assert!(saved.last_proposal_json.is_some());

    // Subsequent successful analysis clears ai_last_error in memory, DB & UI.
    let listener_ok = TcpListener::bind("127.0.0.1:0").expect("bind ok api");
    let addr_ok = listener_ok.local_addr().expect("addr ok");
    let server_ok = std::thread::spawn(move || {
        let (mut stream2, _) = listener_ok.accept().expect("accept ok");
        let _ = read_test_http_request(&mut stream2);
        let proposal_json = serde_json::json!({
            "summary": "All clean",
            "items": []
        })
        .to_string();
        let chunk1 = serde_json::json!({
            "id": "c-ok",
            "object": "chat.completion.chunk",
            "model": "gpt-test",
            "choices": [{
                "index": 0,
                "delta": { "role": "assistant", "content": proposal_json },
                "finish_reason": null
            }]
        });
        let chunk2 = serde_json::json!({
            "id": "c-ok",
            "object": "chat.completion.chunk",
            "model": "gpt-test",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }]
        });
        let sse =
            format!("data: {chunk1}\n\ndata: {chunk2}\n\ndata: [DONE]\n\n");
        write_test_http_response(
            &mut stream2,
            "200 OK",
            "text/event-stream",
            &sse,
        );
    });
    update(&view, win_cx, |app, cx| {
        app.ai_config.api_base_url = format!("http://{addr_ok}/v1");
        app.run_ai_analysis_now(cx);
    });
    win_cx.run_until_parked();
    server_ok.join().expect("server ok join");

    read(&view, win_cx, |app| {
        assert_eq!(app.ai_last_error, None);
        assert_eq!(app.ai_last_error_at_secs, None);
    });
    draw(win_cx);
    assert!(win_cx.debug_bounds("watchdog-ai-last-error").is_none());
    let cleared = StorageDb::open(&db_path)
        .expect("reopen db after success")
        .load_ai_state()
        .expect("load ai state after success");
    assert_eq!(cleared.last_error, None);
    assert_eq!(cleared.last_error_at_secs, None);

    // 2. Main window closed while AI analysis is in flight: still emits
    //    NOTIF_TAG_AI_ANALYSIS and persists error to db_path.
    let db_path2 = cfg_dir.path().join("ruum-closed.db");
    let listener2 = TcpListener::bind("127.0.0.1:0").expect("bind mock api 2");
    let addr2 = listener2.local_addr().expect("addr2");
    let server2 = std::thread::spawn(move || {
        let (mut stream, _) = listener2.accept().expect("accept2");
        let _ = read_test_http_request(&mut stream);
        write_test_http_response(
            &mut stream,
            "422 Unprocessable Entity",
            "application/json",
            r#"{"error":{"message":"model backend crashed"}}"#,
        );
    });

    let root2 = temp.path().to_path_buf();
    let tree2 = scan(temp.path(), options()).expect("scan");
    let db_for_win = db_path2.clone();
    let store_for_win = store;
    let win2 = cx.add_window(move |_, cx| {
        let mut app = Ruum::with_tree(root2.clone(), tree2, options(), 3, cx);
        app.db_path = Some(db_for_win.clone());
        app.cred_store = Some(store_for_win.clone());
        app.set_locale(Locale::En);
        app.ai_config.provider = AiCliProvider::Api;
        app.watchdog_draft.ai_provider = AiCliProvider::Api;
        app.ai_config.api_base_url = format!("http://{addr2}/v1");
        app.ai_config.model = Some("gpt-test".into());
        app.watchdog_draft.model = Some("gpt-test".into());
        app.watchdog_input_api_base_url = None;
        app.watchdog_input_model = None;
        app
    });

    win2.update(cx, |app, _, cx| {
        app.run_ai_analysis_now(cx);
    })
    .expect("start analysis");
    win2.update(cx, |_, window, _| {
        window.remove_window();
    })
    .expect("close win2");
    cx.run_until_parked();
    server2.join().expect("server2 join");

    let shown_after = cx.shown_system_notifications();
    assert_eq!(shown_after.len(), 2);
    assert_eq!(shown_after[1].tag.as_ref(), NOTIF_TAG_AI_ANALYSIS);
    assert_eq!(
        shown_after[1].title.as_ref(),
        crate::i18n::messages::ai_analysis_failed_notif_title(Locale::En)
    );
    assert!(shown_after[1].body.chars().count() <= 160);

    let saved2 = StorageDb::open(&db_path2)
        .expect("open db2")
        .load_ai_state()
        .expect("load ai state 2");
    assert_eq!(
        saved2.last_error.as_deref(),
        Some(shown_after[1].body.as_ref())
    );
    assert!(saved2.last_error_at_secs.is_some());
}

#[gpui_kit::test]
fn remote_ssh_ai_analysis_failure_notifies_and_persists_open_and_closed(
    cx: &mut TestAppContext,
) {
    use ruum_core::storage::{AuthMode, SshProfile, StorageDb};
    use ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS;

    cx.update(gpui_omarchy::init);
    cx.update(|cx| crate::install_notification_handler(cx));
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum-ssh.db");
    let enc_path = cfg_dir.path().join("creds.enc");
    let counter = CountingRootSecret::new([0x52; 32]);

    let profile = SshProfile {
        id: "srv-ai-err".into(),
        alias: "broken-box".into(),
        host: "127.0.0.1".into(),
        port: 22,
        username: "deploy".into(),
        auth_mode: AuthMode::Password,
        default_path: Some("~".into()),
        created_at_secs: 1,
        updated_at_secs: 1,
        last_total_bytes: None,
        last_available_bytes: None,
        last_scanned_secs: None,
        pending_script_path: None,
        pending_script_items: None,
        pending_reclaim_bytes: None,
        pending_created_secs: None,
    };

    // 1. Open main window: failure sends Toast, NOTIF_TAG_AI_ANALYSIS, and
    //    records ai_state.last_error.
    let (view, win_cx) = view_over(temp.path(), cx);
    update(&view, win_cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.cred_store = Some(counter.store_for(&enc_path));
        app.ssh_profiles = vec![profile.clone()];
        app.analyze_ssh_profile("srv-ai-err", cx);
    });
    win_cx.run_until_parked();

    read(&view, win_cx, |app| {
        assert!(!app.ssh_analyzing_profiles.contains("srv-ai-err"));
        let (msg, status) = app.notice.as_ref().expect("error toast");
        assert_eq!(*status, gpui_omarchy::Status::Error);
        assert!(msg.contains("broken-box"));
        assert!(
            app.ai_last_error
                .as_deref()
                .unwrap_or("")
                .contains("broken-box")
        );
    });
    update(&view, win_cx, |app, cx| {
        app.navigate_rail(Screen::Watchdog, cx);
    });
    draw(win_cx);
    assert!(win_cx.debug_bounds("watchdog-ai-last-error").is_some());

    let shown = win_cx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].tag.as_ref(), NOTIF_TAG_AI_ANALYSIS);
    assert!(shown[0].body.contains("broken-box"));

    let saved = StorageDb::open(&db_path)
        .expect("open db")
        .load_ai_state()
        .expect("load ai state");
    assert!(
        saved
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("broken-box")
    );
    assert!(saved.last_error_at_secs.is_some());

    // 2. Closed main window while task runs: still emits NOTIF_TAG_AI_ANALYSIS
    //    and writes ai_state.last_error.
    let db_path2 = cfg_dir.path().join("ruum-ssh-closed.db");
    let root2 = temp.path().to_path_buf();
    let tree2 = scan(temp.path(), options()).expect("scan");
    let db_for_win = db_path2.clone();
    let store2 = counter.store_for(&enc_path);
    let win2 = cx.add_window(move |_, cx| {
        let mut app = Ruum::with_tree(root2.clone(), tree2, options(), 3, cx);
        app.db_path = Some(db_for_win.clone());
        app.cred_store = Some(store2.clone());
        app.ssh_profiles = vec![profile];
        app
    });
    win2.update(cx, |app, _, cx| {
        app.analyze_ssh_profile("srv-ai-err", cx);
    })
    .expect("start remote analysis");
    win2.update(cx, |_, window, _| {
        window.remove_window();
    })
    .expect("close win2 before task finishes");
    cx.run_until_parked();

    let shown_after = cx.shown_system_notifications();
    assert_eq!(shown_after.len(), 2);
    assert_eq!(shown_after[1].tag.as_ref(), NOTIF_TAG_AI_ANALYSIS);
    assert!(shown_after[1].body.contains("broken-box"));

    let saved2 = StorageDb::open(&db_path2)
        .expect("open db2")
        .load_ai_state()
        .expect("load ai state 2");
    assert!(
        saved2
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("broken-box")
    );
    assert!(saved2.last_error_at_secs.is_some());
}

#[gpui_kit::test]
fn watchdog_runtime_ai_failure_notification_notifies_and_toasts_open_window(
    cx: &mut TestAppContext,
) {
    use crate::watchdog_runtime::WatchdogIpcEvent;
    use ruum_core::storage::StorageDb;
    use ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS;
    use std::sync::mpsc::channel;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum-watchdog-runtime.db");
    {
        let db = StorageDb::open(&db_path).expect("open db");
        db.record_ai_error(
            "AI analysis timed out: upstream timeout",
            1_700_000_111,
        )
        .expect("record initial error");
    }

    let tree = scan(temp.path(), options()).expect("scan");
    let root_path = temp.path().to_path_buf();
    let db_for_win = db_path.clone();
    let win = cx.add_window(move |_, cx| {
        let mut app =
            Ruum::with_tree(root_path.clone(), tree, options(), 3, cx);
        app.db_path = Some(db_for_win);
        app
    });

    let (ipc_tx, ipc_rx) = channel();
    cx.update(|cx| {
        crate::install_notification_handler(cx);
        crate::install_test_runtime(
            temp.path().to_path_buf(),
            false,
            Some(win),
            cx,
        );
        crate::with_runtime_mut(cx, |rt| {
            rt.ipc_rx = Some(ipc_rx);
            let _ = rt.in_proc_notif_tx.send((
                NOTIF_TAG_AI_ANALYSIS.to_owned(),
                "ruum AI Analysis Timed Out".to_owned(),
                "AI analysis timed out: upstream timeout".to_owned(),
            ));
        });
        crate::poll_app_runtime(cx);
    });

    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].tag.as_ref(), NOTIF_TAG_AI_ANALYSIS);
    assert_eq!(shown[0].title.as_ref(), "ruum AI Analysis Timed Out");
    assert_eq!(
        shown[0].body.as_ref(),
        "AI analysis timed out: upstream timeout"
    );
    win.read_with(cx, |app, _| {
        assert_eq!(
            app.notice.as_ref(),
            Some(&(
                "AI analysis timed out: upstream timeout".to_owned(),
                gpui_omarchy::Status::Error,
            ))
        );
        assert_eq!(
            app.ai_last_error.as_deref(),
            Some("AI analysis timed out: upstream timeout")
        );
        assert_eq!(app.ai_last_error_at_secs, Some(1_700_000_111));
    })
    .expect("read open window after in_proc_notif");

    // External watchdog daemon IPC ShowNotification also triggers toast + reload.
    win.update(cx, |app, _, _| {
        app.notice = None;
        app.ai_last_error = None;
        app.ai_last_error_at_secs = None;
    })
    .expect("reset window state");
    StorageDb::open(&db_path)
        .expect("reopen db")
        .record_ai_error("AI analysis failed: cli crashed", 1_700_000_222)
        .expect("record second error");
    ipc_tx
        .send(WatchdogIpcEvent::ShowNotification {
            tag: NOTIF_TAG_AI_ANALYSIS.to_owned(),
            title: "ruum AI Analysis Failed".to_owned(),
            body: "AI analysis failed: cli crashed".to_owned(),
        })
        .expect("send ipc notification");
    cx.update(crate::poll_app_runtime);

    let shown_after = cx.shown_system_notifications();
    assert_eq!(shown_after.len(), 2);
    assert_eq!(shown_after[1].tag.as_ref(), NOTIF_TAG_AI_ANALYSIS);
    assert_eq!(shown_after[1].title.as_ref(), "ruum AI Analysis Failed");
    assert_eq!(
        shown_after[1].body.as_ref(),
        "AI analysis failed: cli crashed"
    );
    win.read_with(cx, |app, _| {
        assert_eq!(
            app.notice.as_ref(),
            Some(&(
                "AI analysis failed: cli crashed".to_owned(),
                gpui_omarchy::Status::Error,
            ))
        );
        assert_eq!(
            app.ai_last_error.as_deref(),
            Some("AI analysis failed: cli crashed")
        );
        assert_eq!(app.ai_last_error_at_secs, Some(1_700_000_222));
    })
    .expect("read open window after ipc ShowNotification");
}

#[gpui_kit::test]
fn tray_background_ai_analysis_failure_notifies_persists_and_toasts_reopened_window(
    cx: &mut TestAppContext,
) {
    use crate::i18n::Locale;
    use ruum_core::storage::{AiConfig, StorageDb};
    use ruum_core::watchdog::NOTIF_TAG_AI_ANALYSIS;
    use ruum_core::watchdog_config::{AiCliProvider, ApiProtocol};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum-tray.db");
    let enc_path = cfg_dir.path().join("creds.enc");
    let counter = CountingRootSecret::new([0x53; 32]);
    let store = counter.store_for(&enc_path);
    store.set_ai_api_key("test-key").expect("save api key");

    let listener_504 = TcpListener::bind("127.0.0.1:0").expect("bind 504");
    let addr_504 = listener_504.local_addr().expect("addr 504");
    let stop_504 = Arc::new(AtomicBool::new(false));
    let stop_504_srv = Arc::clone(&stop_504);
    let server_504 = std::thread::spawn(move || {
        while !stop_504_srv.load(Ordering::Acquire) {
            let Ok((mut stream, _)) = listener_504.accept() else {
                break;
            };
            if stop_504_srv.load(Ordering::Acquire) {
                break;
            }
            let _ = read_test_http_request(&mut stream);
            write_test_http_response(
                &mut stream,
                "504 Gateway Timeout",
                "application/json",
                r#"{"error":{"message":"upstream request timed out"}}"#,
            );
        }
    });

    {
        let db = StorageDb::open(&db_path).expect("open db");
        db.save_ai_config(&AiConfig {
            provider: AiCliProvider::Api,
            api_protocol: ApiProtocol::OpenAi,
            api_base_url: format!("http://{addr_504}/v1"),
            model: Some("gpt-test".into()),
            ..AiConfig::default()
        })
        .expect("save ai config");
    }

    let store_rt = store;
    cx.update(|cx| {
        crate::install_notification_handler(cx);
        crate::install_test_runtime(temp.path().to_path_buf(), false, None, cx);
        crate::with_runtime_mut(cx, |rt| {
            rt.cred_store = Some(store_rt);
        });
        crate::trigger_background_ai_analysis_with(
            Some(db_path.clone()),
            Some(temp.path().to_path_buf()),
            Locale::Zh,
            cx,
        );
    });
    cx.run_until_parked();
    stop_504.store(true, Ordering::Release);
    let _ = TcpStream::connect(addr_504);
    server_504.join().expect("server 504 join");

    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].tag.as_ref(), NOTIF_TAG_AI_ANALYSIS);
    assert_eq!(
        shown[0].title.as_ref(),
        crate::i18n::messages::ai_analysis_timed_out_notif_title(Locale::Zh)
    );
    let saved = StorageDb::open(&db_path)
        .expect("reopen db")
        .load_ai_state()
        .expect("load ai state");
    assert_eq!(saved.last_error.as_deref(), Some(shown[0].body.as_ref()));
    assert!(saved.last_error_at_secs.is_some());
    assert!(cx.read(|cx| {
        !cx.global::<crate::AppRuntime>()
            .background_analysis_in_flight
    }));

    // 2. Main window opens while background AI analysis is running:
    //    emits notification, persists error, reloads ai_last_error, and
    //    pushes Status::Error toast to the opened main window.
    let listener_422 = TcpListener::bind("127.0.0.1:0").expect("bind 422");
    let addr_422 = listener_422.local_addr().expect("addr 422");
    let server_422 = std::thread::spawn(move || {
        let (mut stream, _) = listener_422.accept().expect("accept 422");
        let _ = read_test_http_request(&mut stream);
        write_test_http_response(
            &mut stream,
            "422 Unprocessable Entity",
            "application/json",
            r#"{"error":{"message":"model backend crashed"}}"#,
        );
    });
    {
        let db = StorageDb::open(&db_path).expect("reopen db 2");
        db.clear_ai_error(1_700_000_333).expect("clear err");
        db.save_ai_config(&AiConfig {
            provider: AiCliProvider::Api,
            api_protocol: ApiProtocol::OpenAi,
            api_base_url: format!("http://{addr_422}/v1"),
            model: Some("gpt-test".into()),
            ..AiConfig::default()
        })
        .expect("save 422 ai config");
    }

    cx.update(|cx| {
        crate::trigger_background_ai_analysis_with(
            Some(db_path.clone()),
            Some(temp.path().to_path_buf()),
            Locale::En,
            cx,
        );
    });

    let root_reopen = temp.path().to_path_buf();
    let tree_reopen = scan(temp.path(), options()).expect("scan");
    let db_reopen = db_path.clone();
    let win = cx.add_window(move |_, cx| {
        let mut app =
            Ruum::with_tree(root_reopen.clone(), tree_reopen, options(), 3, cx);
        app.db_path = Some(db_reopen);
        app.set_locale(Locale::En);
        app
    });
    cx.update(|cx| {
        crate::with_runtime_mut(cx, |rt| rt.main_window = Some(win));
    });
    cx.run_until_parked();
    server_422.join().expect("server 422 join");

    let shown_after = cx.shown_system_notifications();
    assert_eq!(shown_after.len(), 2);
    assert_eq!(shown_after[1].tag.as_ref(), NOTIF_TAG_AI_ANALYSIS);
    assert_eq!(
        shown_after[1].title.as_ref(),
        crate::i18n::messages::ai_analysis_failed_notif_title(Locale::En)
    );
    let expected_body = shown_after[1].body.to_string();

    let saved2 = StorageDb::open(&db_path)
        .expect("reopen db after 422")
        .load_ai_state()
        .expect("load ai state 2");
    assert_eq!(saved2.last_error.as_deref(), Some(expected_body.as_str()));
    assert!(saved2.last_error_at_secs.is_some());

    win.read_with(cx, |app, _| {
        assert_eq!(
            app.notice.as_ref(),
            Some(&(expected_body.clone(), gpui_omarchy::Status::Error))
        );
        assert_eq!(app.ai_last_error.as_deref(), Some(expected_body.as_str()));
        assert!(app.ai_last_error_at_secs.is_some());
    })
    .expect("read reopened main window toast and error");
}

#[gpui_kit::test]
fn settings_general_persists_background_and_autostart_without_legacy_service_toggle(
    cx: &mut TestAppContext,
) {
    use gpui_kit::Modifiers;
    use ruum_core::storage::StorageDb;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");

    let (view, cx) = view_over(temp.path(), cx);
    cx.simulate_resize(gpui_kit::size(px(1100.), px(1400.)));
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(cx);

    let close_bg_box = cx
        .debug_bounds("setting-close-to-background")
        .expect("close-to-background toggle");
    let autostart_box = cx
        .debug_bounds("setting-launch-at-login")
        .expect("launch-at-login toggle");
    cx.simulate_click(close_bg_box.center(), Modifiers::none());
    cx.simulate_click(autostart_box.center(), Modifiers::none());
    draw(cx);

    // Legacy headless service toggle must no longer be rendered in Watchdog.
    let wd_tab = cx.debug_bounds("setting-tab-watchdog").expect("wd tab");
    cx.simulate_click(wd_tab.center(), Modifiers::none());
    draw(cx);
    assert!(cx.debug_bounds("setting-enable-watchdog").is_some());
    assert!(cx.debug_bounds("setting-enable-service").is_none());

    let save_btn = cx.debug_bounds("setting-save-btn").expect("save btn");
    cx.simulate_click(save_btn.center(), Modifiers::none());
    draw(cx);

    let saved = StorageDb::open(&db_path)
        .expect("open db")
        .load_app_settings()
        .expect("load saved settings");
    assert!(!saved.close_to_background);
    assert!(saved.launch_at_login);

    cx.update(|_, cx| {
        crate::app_menu::set_tray_active(cx, true);
        assert!(crate::app_menu::should_quit_on_all_windows_closed(cx));
    });

    let (fresh_view, fresh_cx) = view_over(temp.path(), &mut cx.cx);
    update(&fresh_view, fresh_cx, |app, _| {
        app.db_path = Some(db_path.clone());
        app.reload_watchdog_config_and_prepare_settings();
    });
    read(&fresh_view, fresh_cx, |app| {
        assert!(!app.app_settings.close_to_background);
        assert!(app.app_settings.launch_at_login);
        assert!(!app.watchdog_draft.close_to_background);
        assert!(app.watchdog_draft.launch_at_login);
    });
}

#[gpui_kit::test]
fn tray_toggle_watchdog_flips_db_runtime_and_open_window(
    cx: &mut TestAppContext,
) {
    use ruum_core::storage::StorageDb;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");

    let root = temp.path().to_path_buf();
    let tree = scan(temp.path(), options()).expect("scan");
    let db_for_win = db_path.clone();
    let win = cx.add_window(move |_, cx| {
        let mut app = Ruum::with_tree(root, tree, options(), 3, cx);
        app.db_path = Some(db_for_win);
        app
    });

    cx.update(|cx| {
        crate::install_test_runtime(
            temp.path().to_path_buf(),
            false,
            Some(win),
            cx,
        );
        crate::toggle_watchdog_in_db(Some(&db_path), cx);
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.watchdog_config.enabled);
    });

    let enabled_in_db = StorageDb::open(&db_path)
        .unwrap()
        .watchdog()
        .unwrap()
        .load_config()
        .unwrap()
        .enabled;
    assert!(enabled_in_db);
    win.read_with(cx, |app, _| {
        assert!(app.watchdog_config.enabled);
        assert!(app.watchdog_draft.enabled);
    })
    .expect("read enabled main window");

    cx.update(|cx| {
        crate::toggle_watchdog_in_db(Some(&db_path), cx);
        let rt = cx.global::<crate::AppRuntime>();
        assert!(!rt.watchdog_config.enabled);
    });

    let disabled_in_db = StorageDb::open(&db_path)
        .unwrap()
        .watchdog()
        .unwrap()
        .load_config()
        .unwrap()
        .enabled;
    assert!(!disabled_in_db);
    win.read_with(cx, |app, _| {
        assert!(!app.watchdog_config.enabled);
        assert!(!app.watchdog_draft.enabled);
    })
    .expect("read disabled main window");
}

#[test]
fn has_actionable_proposal_counts_docker_targets() {
    use ruum_core::docker_adapter::DockerCleanTarget;
    use ruum_core::watchdog_config::CleanupProposal;

    let empty = CleanupProposal {
        timestamp_secs: 1,
        summary: "none".to_owned(),
        items: Vec::new(),
        docker_targets: Vec::new(),
    };
    assert!(!crate::has_actionable_proposal(&empty));

    let docker_only = CleanupProposal {
        timestamp_secs: 1,
        summary: "docker".to_owned(),
        items: Vec::new(),
        docker_targets: vec![DockerCleanTarget::BuildCache],
    };
    assert!(crate::has_actionable_proposal(&docker_only));
}

#[gpui_kit::test]
#[allow(
    clippy::needless_pass_by_ref_mut,
    reason = "gpui_kit::test passes &mut TestAppContext"
)]
fn request_quit_cancels_pending_ipc_confirm_windows_without_deadlock(
    cx: &mut TestAppContext,
) {
    use ruum_core::confirm::{
        CleanupConfirmDecision, CleanupConfirmRequest, IpcListenerGuard,
        IpcRequest, IpcResponse, RemoteConnectConfirmDecision,
        RemoteConnectConfirmRequest, send_ipc_message,
    };

    cx.update(gpui_omarchy::init);
    let temp = tempfile::tempdir().expect("tempdir");
    #[cfg(unix)]
    let socket_or_port = temp.path().join("quit-ipc.sock");
    #[cfg(windows)]
    let socket_or_port = temp.path().join("quit-ipc.port");

    let listener = IpcListenerGuard::bind(&socket_or_port).expect("bind");
    let token = "quit_test_token_12345678901234567890".to_owned();
    let (server, rx) =
        crate::watchdog_runtime::WatchdogIpcServer::start_with_listener(
            listener,
            token.clone(),
        )
        .expect("start ipc server");

    cx.update(|cx| {
        crate::install_test_runtime(temp.path().to_path_buf(), false, None, cx);
        crate::with_runtime_mut(cx, |rt| rt.ipc_server = Some(server));
    });

    let ep1 = socket_or_port.clone();
    let tok1 = token.clone();
    let confirm_handle = std::thread::spawn(move || {
        send_ipc_message(
            &ep1,
            &IpcRequest::ConfirmCleanup {
                token: tok1,
                request: CleanupConfirmRequest {
                    root: PathBuf::from("/tmp"),
                    title: "Pending Confirm".to_owned(),
                    summary: None,
                    items: Vec::new(),
                    docker_items: Vec::new(),
                    docker_permission_hint: None,
                },
            },
        )
    });

    let ep2 = socket_or_port;
    let remote_handle = std::thread::spawn(move || {
        send_ipc_message(
            &ep2,
            &IpcRequest::ConfirmRemoteConnect {
                token,
                request: RemoteConnectConfirmRequest {
                    target_label: "prod".to_owned(),
                    alias: Some("prod".to_owned()),
                    host: "10.0.0.1".to_owned(),
                    port: 22,
                    username: "deploy".to_owned(),
                    auth_mode: "private_key".to_owned(),
                    tool_name: "scan_directory".to_owned(),
                },
            },
        )
    });

    let mut clean_ev = None;
    let mut remote_ev = None;
    for _ in 0..2 {
        match rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("ipc ev")
        {
            crate::watchdog_runtime::WatchdogIpcEvent::Confirm {
                request,
                responder,
            } => clean_ev = Some((request, responder)),
            crate::watchdog_runtime::WatchdogIpcEvent::ConfirmRemoteConnect {
                request,
                responder,
            } => remote_ev = Some((request, responder)),
            other => panic!("unexpected ipc event: {other:?}"),
        }
    }

    let (c_req, c_resp) = clean_ev.expect("clean ev");
    let (r_req, r_resp) = remote_ev.expect("remote ev");

    cx.update(|cx| {
        crate::open_confirm_window(c_req, Some(c_resp), None, cx);
        crate::open_remote_connect_confirm_window_with_mode(
            r_req,
            Some(r_resp),
            None,
            false,
            cx,
        );
        crate::app_menu::request_quit(cx);
        assert!(crate::app_menu::is_quitting(cx));
    });

    assert_eq!(
        confirm_handle.join().unwrap().expect("confirm ipc resp"),
        IpcResponse::ConfirmDecision {
            decision: CleanupConfirmDecision::Cancelled
        }
    );
    assert_eq!(
        remote_handle.join().unwrap().expect("remote ipc resp"),
        IpcResponse::RemoteConnectDecision {
            decision: RemoteConnectConfirmDecision::Denied
        }
    );
}

#[gpui_kit::test]
fn settings_general_persists_tray_show_disk_space_and_syncs_runtime(
    cx: &mut TestAppContext,
) {
    use gpui_kit::Modifiers;
    use ruum_core::storage::StorageDb;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");

    let (view, cx) = view_over(temp.path(), cx);
    cx.update(|_, cx| {
        crate::install_test_runtime(temp.path().to_path_buf(), false, None, cx);
    });
    cx.simulate_resize(gpui_kit::size(px(1100.), px(1400.)));
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(cx);

    cx.update(|_, cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(!rt.tray_show_disk_space);
    });

    let tray_toggle = cx
        .debug_bounds("setting-tray-disk-space")
        .expect("tray disk space toggle");
    cx.simulate_click(tray_toggle.center(), Modifiers::none());
    draw(cx);

    let save_btn = cx.debug_bounds("setting-save-btn").expect("save btn");
    cx.simulate_click(save_btn.center(), Modifiers::none());
    draw(cx);

    let saved = StorageDb::open(&db_path)
        .expect("open db")
        .load_app_settings()
        .expect("load saved settings");
    assert!(saved.tray_show_disk_space);
    cx.update(|_, cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.tray_show_disk_space);
    });

    // Fresh Ruum reloads tray_show_disk_space from SQLite into settings & draft.
    let (fresh_view, fresh_cx) = view_over(temp.path(), &mut cx.cx);
    fresh_cx.simulate_resize(gpui_kit::size(px(1100.), px(1400.)));
    update(&fresh_view, fresh_cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.reload_watchdog_config_and_prepare_settings();
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(fresh_cx);
    read(&fresh_view, fresh_cx, |app| {
        assert!(app.app_settings.tray_show_disk_space);
        assert!(app.watchdog_draft.tray_show_disk_space);
    });

    // Toggling back off persists false and clears AppRuntime cache on save.
    let tray_toggle_off = fresh_cx
        .debug_bounds("setting-tray-disk-space")
        .expect("tray disk space toggle off");
    fresh_cx.simulate_click(tray_toggle_off.center(), Modifiers::none());
    draw(fresh_cx);
    let save_btn_2 = fresh_cx
        .debug_bounds("setting-save-btn")
        .expect("save btn 2");
    fresh_cx.simulate_click(save_btn_2.center(), Modifiers::none());
    draw(fresh_cx);

    let saved_off = StorageDb::open(&db_path)
        .expect("reopen db")
        .load_app_settings()
        .expect("load saved settings off");
    assert!(!saved_off.tray_show_disk_space);
    fresh_cx.update(|_, cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(!rt.tray_show_disk_space);
    });
}

#[gpui_kit::test]
fn settings_general_tray_disk_space_waits_for_save_before_persisting_and_applying(
    cx: &mut TestAppContext,
) {
    use gpui_kit::Modifiers;
    use ruum_core::storage::StorageDb;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");

    let (view, cx) = view_over(temp.path(), cx);
    cx.update(|_, cx| {
        crate::install_test_runtime(temp.path().to_path_buf(), false, None, cx);
    });
    cx.simulate_resize(gpui_kit::size(px(1100.), px(1400.)));
    update(&view, cx, |app, cx| {
        app.db_path = Some(db_path.clone());
        app.navigate_rail(Screen::Settings, cx);
    });
    draw(cx);

    // 1. Toggling `setting-tray-disk-space` updates only draft until Save.
    let tray_toggle = cx
        .debug_bounds("setting-tray-disk-space")
        .expect("tray disk space toggle");
    cx.simulate_click(tray_toggle.center(), Modifiers::none());
    draw(cx);

    read(&view, cx, |app| {
        assert!(app.watchdog_draft.tray_show_disk_space);
        assert!(!app.app_settings.tray_show_disk_space);
    });
    cx.update(|_, cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(!rt.tray_show_disk_space);
    });
    let before_save = StorageDb::open(&db_path)
        .expect("open db before save")
        .load_app_settings()
        .expect("load settings before save");
    assert!(!before_save.tray_show_disk_space);

    // 2. Clicking Save Settings persists to SQLite, updates state & runtime.
    let save_btn = cx.debug_bounds("setting-save-btn").expect("save btn");
    cx.simulate_click(save_btn.center(), Modifiers::none());
    draw(cx);

    let saved_on = StorageDb::open(&db_path)
        .expect("open db after save")
        .load_app_settings()
        .expect("load saved settings on");
    assert!(saved_on.tray_show_disk_space);
    read(&view, cx, |app| {
        assert!(app.app_settings.tray_show_disk_space);
        assert!(app.watchdog_draft.tray_show_disk_space);
    });
    cx.update(|_, cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.tray_show_disk_space);
    });

    // 3. Toggling back off keeps persisted, applied, and runtime values true
    // until Save Settings is clicked again.
    let tray_toggle_off = cx
        .debug_bounds("setting-tray-disk-space")
        .expect("tray disk space toggle off");
    cx.simulate_click(tray_toggle_off.center(), Modifiers::none());
    draw(cx);

    read(&view, cx, |app| {
        assert!(!app.watchdog_draft.tray_show_disk_space);
        assert!(app.app_settings.tray_show_disk_space);
    });
    cx.update(|_, cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.tray_show_disk_space);
    });
    let before_save_off = StorageDb::open(&db_path)
        .expect("reopen db before save off")
        .load_app_settings()
        .expect("load settings before save off");
    assert!(before_save_off.tray_show_disk_space);

    // 4. Clicking Save Settings again persists and applies false.
    let save_btn_off =
        cx.debug_bounds("setting-save-btn").expect("save btn off");
    cx.simulate_click(save_btn_off.center(), Modifiers::none());
    draw(cx);

    read(&view, cx, |app| {
        assert!(!app.app_settings.tray_show_disk_space);
        assert!(!app.watchdog_draft.tray_show_disk_space);
    });
    cx.update(|_, cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(!rt.tray_show_disk_space);
    });
    let saved_off = StorageDb::open(&db_path)
        .expect("reopen db after save off")
        .load_app_settings()
        .expect("load saved settings off");
    assert!(!saved_off.tray_show_disk_space);
}

#[gpui_kit::test]
#[allow(
    clippy::needless_pass_by_ref_mut,
    reason = "gpui_kit::test passes &mut TestAppContext"
)]
fn tray_space_refresh_throttle_only_polls_when_enabled_and_due(
    cx: &mut TestAppContext,
) {
    use std::time::{Duration, Instant};

    let t0 = Instant::now();
    let t29 = t0 + Duration::from_secs(29);
    let t30 = t0 + Duration::from_secs(30);
    assert!(!crate::should_refresh_tray_disk_space(
        false, true, None, t0
    ));
    assert!(!crate::should_refresh_tray_disk_space(
        true, false, None, t0
    ));
    assert!(crate::should_refresh_tray_disk_space(true, true, None, t0));
    assert!(!crate::should_refresh_tray_disk_space(
        true,
        true,
        Some(t0),
        t29,
    ));
    assert!(crate::should_refresh_tray_disk_space(
        true,
        true,
        Some(t0),
        t30,
    ));

    // When disabled, poll_app_runtime must not probe or stamp refresh time.
    let temp = tempfile::tempdir().expect("tempdir");
    cx.update(|cx| {
        crate::install_test_runtime(temp.path().to_path_buf(), false, None, cx);
        crate::poll_app_runtime(cx);
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.last_tray_space_refresh.is_none());
    });
}

#[gpui_kit::test]
fn cleanup_progress_window_opens_on_confirm_background_keeps_running_tray_reopens_and_cancel_stops(
    cx: &mut TestAppContext,
) {
    use ruum_core::storage::{AiState, StorageDb};
    use ruum_core::watchdog::NOTIF_TAG_CLEANUP_DONE;
    use ruum_core::watchdog_config::{CleanupProposal, ProposalItem};
    use std::sync::atomic::Ordering;

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");

    let file_a = temp.path().join("junk/cancel_a.bin");
    let file_b = temp.path().join("junk/cancel_b.bin");
    std::fs::write(&file_a, vec![b'a'; 1024]).expect("write a");
    std::fs::write(&file_b, vec![b'b'; 2048]).expect("write b");

    let proposal = CleanupProposal {
        timestamp_secs: 1_700_000_000,
        summary: "Test cancellable cleanup".into(),
        items: vec![
            ProposalItem::new(
                file_a.clone(),
                1024,
                RemovalMode::Permanent,
                Some("stale".into()),
                None,
            ),
            ProposalItem::new(
                file_b.clone(),
                2048,
                RemovalMode::Permanent,
                Some("stale".into()),
                None,
            ),
        ],
        docker_targets: Vec::new(),
    };
    {
        let db = StorageDb::open(&db_path).expect("open db");
        let st = AiState::from_proposal(&proposal, 1_700_000_000).expect("st");
        db.save_ai_state(&st).expect("save ai state");
    }

    let tree = scan(temp.path(), options()).expect("scan fixture");
    let root_path = temp.path().to_path_buf();
    let db_for_win = db_path.clone();
    let main_win = cx.add_window(move |_, cx| {
        let mut app = Ruum::with_tree(root_path, tree, options(), 3, cx);
        app.locale = crate::i18n::Locale::En;
        app.db_path = Some(db_for_win);
        app.reload_watchdog_state();
        app
    });
    let cleanup_gate = std::sync::Arc::new(crate::AsyncSignal::default());
    let cleanup_gate_rt = std::sync::Arc::clone(&cleanup_gate);
    cx.update(|cx| {
        crate::install_notification_handler(cx);
        crate::app_menu::install_for(crate::i18n::Locale::En, cx);
        crate::install_test_runtime(
            temp.path().to_path_buf(),
            false,
            Some(main_win),
            cx,
        );
        crate::with_runtime_mut(cx, |rt| {
            rt.test_cleanup_gate = Some(cleanup_gate_rt);
        });
    });

    let req = crate::state::proposal_confirm_request(
        temp.path().to_path_buf(),
        &proposal,
        crate::i18n::Locale::En,
    );
    let confirm_win = cx
        .update(|cx| {
            crate::open_local_confirm_window(
                req,
                Some(db_path),
                ruum_core::docker_adapter::DockerProbeStatus::NotInstalled,
                cx,
            )
        })
        .expect("open local confirm window");
    let _ = confirm_win.update(cx, |win, _, _| {
        win.set_locale(crate::i18n::Locale::En);
    });
    let mut confirm_cx = VisualTestContext::from_window(confirm_win.into(), cx);
    draw(&mut confirm_cx);
    let confirm_btn = confirm_cx
        .debug_bounds("confirm-clean")
        .expect("confirm button");
    confirm_cx
        .simulate_click(confirm_btn.center(), gpui_kit::Modifiers::none());

    // Immediately after confirm (before background executor drains):
    // confirm window closed, active_cleanup registered, CleanupProgressWindow opened.
    let (progress_win, cancel_arc) = cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        let prog_win = rt
            .cleanup_progress_window
            .expect("CleanupProgressWindow opened on confirm");
        let task = rt
            .active_cleanup
            .as_ref()
            .expect("ActiveCleanupTask stored on AppRuntime");
        (prog_win, std::sync::Arc::clone(&task.cancel))
    });
    assert!(!cancel_arc.load(Ordering::Acquire));
    assert_eq!(
        cx.read(crate::current_tray_cleanup_status),
        Some(crate::tray_menu::TrayCleanupStatus {
            completed: 0,
            total: 2,
            cancelling: false,
        })
    );

    let mut prog_cx = VisualTestContext::from_window(progress_win.into(), cx);
    draw(&mut prog_cx);
    assert!(prog_cx.debug_bounds("cleanup-progress-spinner").is_some());
    assert!(prog_cx.debug_bounds("cleanup-progress-bar").is_some());
    assert!(prog_cx.debug_bounds("cleanup-progress-counter").is_some());
    assert!(prog_cx.debug_bounds("cleanup-progress-path").is_some());
    let bg_btn = prog_cx
        .debug_bounds("cleanup-progress-background")
        .expect("background continue button");
    assert!(prog_cx.debug_bounds("cleanup-progress-cancel").is_some());

    // Clicking "Continue in Background" closes the window without cancelling.
    prog_cx.simulate_click(bg_btn.center(), gpui_kit::Modifiers::none());
    cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.cleanup_progress_window.is_none());
        assert!(rt.active_cleanup.is_some());
    });
    assert!(!cancel_arc.load(Ordering::Acquire));

    // Tray ShowCleanupProgress reopens CleanupProgressWindow.
    let reopened = cx.update(|cx| {
        crate::handle_tray_action(
            crate::tray_menu::TrayAction::ShowCleanupProgress,
            cx,
        );
        cx.global::<crate::AppRuntime>()
            .cleanup_progress_window
            .expect("reopened progress window")
    });
    let mut reopened_cx = VisualTestContext::from_window(reopened.into(), cx);
    draw(&mut reopened_cx);
    assert!(
        reopened_cx
            .debug_bounds("cleanup-progress-cancel")
            .is_some()
    );

    // CloseWindow shortcut (Cmd+W / Ctrl+W) hides without cancelling either.
    reopened_cx.dispatch_action(crate::app_menu::CloseWindow);
    cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.cleanup_progress_window.is_none());
        assert!(rt.active_cleanup.is_some());
    });
    assert!(!cancel_arc.load(Ordering::Acquire));

    // Reopen once more and click "Cancel Cleanup".
    let reopened2 = cx.update(|cx| {
        crate::handle_tray_action(
            crate::tray_menu::TrayAction::ShowCleanupProgress,
            cx,
        );
        cx.global::<crate::AppRuntime>()
            .cleanup_progress_window
            .expect("reopened2 progress window")
    });
    let mut reopened2_cx = VisualTestContext::from_window(reopened2.into(), cx);
    draw(&mut reopened2_cx);
    let cancel_btn = reopened2_cx
        .debug_bounds("cleanup-progress-cancel")
        .expect("cancel button");
    reopened2_cx
        .simulate_click(cancel_btn.center(), gpui_kit::Modifiers::none());
    assert!(cancel_arc.load(Ordering::Acquire));
    assert_eq!(
        cx.read(crate::current_tray_cleanup_status),
        Some(crate::tray_menu::TrayCleanupStatus {
            completed: 0,
            total: 2,
            cancelling: true,
        })
    );
    draw(&mut reopened2_cx);
    // Repeated click while stopping stays safe and disabled.
    reopened2_cx
        .simulate_click(cancel_btn.center(), gpui_kit::Modifiers::none());
    assert!(cancel_arc.load(Ordering::Acquire));

    // Release the gate and drain background cleanup: cancelled before
    // executing items -> files stay, active_cleanup clears,
    // NOTIF_TAG_CLEANUP_DONE sent, and progress window lingers for 3 seconds.
    cleanup_gate.notify();
    cx.run_until_parked();
    assert!(file_a.exists());
    assert!(file_b.exists());
    let prog_win = cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.active_cleanup.is_none());
        rt.cleanup_progress_window
            .expect("progress window lingers after cancellation")
    });
    assert!(cx.read(crate::current_tray_cleanup_status).is_none());
    let notifs = cx.shown_system_notifications();
    assert_eq!(
        notifs.last().map(|n| n.tag.as_ref()),
        Some(NOTIF_TAG_CLEANUP_DONE)
    );
    assert!(
        notifs
            .last()
            .is_some_and(|n| n.body.to_lowercase().contains("cancel"))
    );

    let mut done_cx = VisualTestContext::from_window(prog_win.into(), cx);
    draw(&mut done_cx);
    assert!(done_cx.debug_bounds("cleanup-progress-done-icon").is_some());
    assert!(done_cx.debug_bounds("cleanup-progress-close").is_some());
    assert!(done_cx.debug_bounds("cleanup-progress-cancel").is_none());

    cx.background_executor
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.cleanup_progress_window.is_some());
    });
    cx.background_executor
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.cleanup_progress_window.is_none());
    });
}

#[gpui_kit::test]
fn cleanup_progress_window_renders_under_csd_and_refreshes_main_window_on_completion(
    cx: &mut TestAppContext,
) {
    use gpui_kit::{
        AppContext as _, Decorations, Modifiers, ParentElement as _, Render,
        Styled as _, Tiling, WindowControls, div, size,
    };
    use ruum_core::storage::{AiState, StorageDb};
    use ruum_core::watchdog_config::{CleanupProposal, ProposalItem};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct CsdProgressHost {
        inner: Entity<crate::CleanupProgressWindow>,
    }

    impl Render for CsdProgressHost {
        fn render(
            &mut self,
            window: &mut gpui_kit::Window,
            _: &mut Context<'_, Self>,
        ) -> impl gpui_kit::IntoElement {
            let state = crate::window_chrome::WindowChromeState {
                decorations: Decorations::Client {
                    tiling: Tiling::default(),
                },
                controls: WindowControls {
                    fullscreen: false,
                    maximize: false,
                    minimize: true,
                    window_menu: true,
                },
                button_layout:
                    crate::window_chrome::default_linux_button_layout(),
                is_maximized: false,
                is_window_active: true,
                window_size: window.bounds().size,
            };
            let entity = self.inner.clone();
            let actions = crate::window_chrome::WindowChromeActions::new(
                move |window, cx| {
                    crate::CleanupProgressWindow::close_from_chrome(
                        &entity.downgrade(),
                        window,
                        cx,
                    );
                },
            );
            crate::window_chrome::wrap_window_chrome_with(
                "CSD Cleanup Progress",
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .size_full()
                    .child(self.inner.clone()),
                state,
                actions,
            )
        }
    }

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(crate::CleanupProgressSnapshot {
        total_items: 4,
        total_bytes: 4096,
        completed_items: 2,
        reclaimed_bytes: 2048,
        current_item: Some("~/junk/very/long/nested/path/cache.bin".into()),
        cancelling: false,
        completion: None,
    }));
    let cancel_check = Arc::clone(&cancel);
    let (host_win, host_cx) = cx.add_window_view(move |_, cx| {
        let inner = cx
            .new(|cx| crate::CleanupProgressWindow::new(cancel, progress, cx));
        CsdProgressHost { inner }
    });
    host_cx.simulate_resize(size(px(540.0), px(300.0)));
    draw(host_cx);
    let bg_b = host_cx
        .debug_bounds("cleanup-progress-background")
        .expect("bg button at default");
    let cancel_b = host_cx
        .debug_bounds("cleanup-progress-cancel")
        .expect("cancel button at default");
    let bar_b = host_cx.debug_bounds("csd-title-bar").expect("csd titlebar");
    assert!(bg_b.top() >= bar_b.bottom());
    assert!(bg_b.bottom() <= px(300.0));
    assert!(cancel_b.bottom() <= px(300.0));

    // Verify rendering across all 4 locales at minimum size 460x280.
    host_cx.simulate_resize(size(px(460.0), px(280.0)));
    for locale in [
        crate::i18n::Locale::En,
        crate::i18n::Locale::Zh,
        crate::i18n::Locale::Ja,
        crate::i18n::Locale::Ko,
    ] {
        host_win.update(host_cx, |host, cx| {
            host.inner.update(cx, |win, cx| {
                win.set_locale(locale);
                cx.notify();
            });
        });
        draw(host_cx);
        let min_cancel_b = host_cx
            .debug_bounds("cleanup-progress-cancel")
            .expect("cancel button at min size");
        assert!(min_cancel_b.bottom() <= px(280.0));
    }

    // Closing via Linux CSD close button must NOT set cancel.
    let close_btn = host_cx
        .debug_bounds("csd-btn-close")
        .expect("csd close button");
    host_cx.simulate_click(close_btn.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(!cancel_check.load(Ordering::Acquire));
    drop(host_win);

    // End-to-end completion refreshes main window watchdog_home_space,
    // ai_proposal, DB, and clears active_cleanup.
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let junk_file = temp.path().join("junk/done_item.bin");
    std::fs::write(&junk_file, vec![b'z'; 512]).expect("write junk");
    let proposal = CleanupProposal {
        timestamp_secs: 1_700_000_100,
        summary: "Complete test".into(),
        items: vec![ProposalItem::new(
            junk_file.clone(),
            512,
            RemovalMode::Permanent,
            None,
            None,
        )],
        docker_targets: Vec::new(),
    };
    {
        let db = StorageDb::open(&db_path).expect("open db");
        let st = AiState::from_proposal(&proposal, 1_700_000_100).expect("st");
        db.save_ai_state(&st).expect("save st");
    }
    let tree = scan(temp.path(), options()).expect("scan");
    let root_path = temp.path().to_path_buf();
    let db_for_win = db_path.clone();
    let main_win = cx.add_window(move |_, cx| {
        let mut app = Ruum::with_tree(root_path, tree, options(), 3, cx);
        app.db_path = Some(db_for_win);
        app.watchdog_home_space = None;
        app.reload_watchdog_state();
        app
    });
    cx.update(|cx| {
        crate::install_notification_handler(cx);
        crate::install_test_runtime(
            temp.path().to_path_buf(),
            false,
            Some(main_win),
            cx,
        );
    });
    let req = crate::state::proposal_confirm_request(
        temp.path().to_path_buf(),
        &proposal,
        crate::i18n::Locale::En,
    );
    let confirm_win = cx
        .update(|cx| {
            crate::open_local_confirm_window(
                req,
                Some(db_path.clone()),
                ruum_core::docker_adapter::DockerProbeStatus::NotInstalled,
                cx,
            )
        })
        .expect("confirm win");
    let mut confirm_cx = VisualTestContext::from_window(confirm_win.into(), cx);
    draw(&mut confirm_cx);
    let confirm_btn = confirm_cx
        .debug_bounds("confirm-clean")
        .expect("confirm button");
    confirm_cx.simulate_click(confirm_btn.center(), Modifiers::none());
    cx.run_until_parked();

    assert!(!junk_file.exists());
    main_win
        .read_with(cx, |app, _| {
            assert!(app.ai_proposal.is_none());
            assert!(app.watchdog_home_space.is_some());
            assert!(app.watchdog_space_checked_at.is_some());
        })
        .expect("read main win after completion");
    let reloaded = StorageDb::open(&db_path)
        .expect("reopen db")
        .load_ai_state()
        .expect("load ai state");
    assert!(reloaded.last_proposal_json.is_none());
    let prog_win = cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.active_cleanup.is_none());
        rt.cleanup_progress_window
            .expect("progress window lingers in Completed state")
    });
    let mut prog_cx = VisualTestContext::from_window(prog_win.into(), cx);
    draw(&mut prog_cx);
    assert!(prog_cx.debug_bounds("cleanup-progress-done-icon").is_some());
    assert!(prog_cx.debug_bounds("cleanup-progress-close").is_some());
    assert!(prog_cx.debug_bounds("cleanup-progress-cancel").is_none());

    cx.background_executor
        .advance_clock(crate::CLEANUP_PROGRESS_LINGER);
    cx.run_until_parked();
    cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.cleanup_progress_window.is_none());
    });
}

#[gpui_kit::test]
fn cleanup_progress_completion_states_manual_close_background_and_locales(
    cx: &mut TestAppContext,
) {
    use gpui_kit::{
        AppContext as _, Decorations, Modifiers, ParentElement as _, Render,
        Styled as _, Tiling, WindowControls, div, size,
    };
    use ruum_core::storage::{AiState, StorageDb};
    use ruum_core::watchdog_config::{CleanupProposal, ProposalItem};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct CsdDoneHost {
        inner: Entity<crate::CleanupProgressWindow>,
    }

    impl Render for CsdDoneHost {
        fn render(
            &mut self,
            window: &mut gpui_kit::Window,
            _: &mut Context<'_, Self>,
        ) -> impl gpui_kit::IntoElement {
            let state = crate::window_chrome::WindowChromeState {
                decorations: Decorations::Client {
                    tiling: Tiling::default(),
                },
                controls: WindowControls {
                    fullscreen: false,
                    maximize: false,
                    minimize: true,
                    window_menu: true,
                },
                button_layout:
                    crate::window_chrome::default_linux_button_layout(),
                is_maximized: false,
                is_window_active: true,
                window_size: window.bounds().size,
            };
            let entity = self.inner.clone();
            let actions = crate::window_chrome::WindowChromeActions::new(
                move |window, cx| {
                    crate::CleanupProgressWindow::close_from_chrome(
                        &entity.downgrade(),
                        window,
                        cx,
                    );
                },
            );
            crate::window_chrome::wrap_window_chrome_with(
                "CSD Cleanup Done",
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .size_full()
                    .child(self.inner.clone()),
                state,
                actions,
            )
        }
    }

    cx.update(gpui_omarchy::init);
    let temp = fixture();

    // 1. Render Completed, Cancelled, and Failed completion states across all
    //    4 locales at minimum size 460x280 under CSD without footer overflow.
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(crate::CleanupProgressSnapshot {
        total_items: 3,
        total_bytes: 4096,
        completed_items: 3,
        reclaimed_bytes: 4096,
        current_item: None,
        cancelling: false,
        completion: Some(crate::CleanupProgressCompletion::Completed {
            count: 3,
            bytes: 4096,
        }),
    }));
    let progress_handle = Arc::clone(&progress);
    let (host_win, host_cx) = cx.add_window_view(move |_, cx| {
        let inner = cx
            .new(|cx| crate::CleanupProgressWindow::new(cancel, progress, cx));
        CsdDoneHost { inner }
    });
    host_cx.simulate_resize(size(px(460.0), px(280.0)));
    let completions = [
        crate::CleanupProgressCompletion::Completed {
            count: 3,
            bytes: 4096,
        },
        crate::CleanupProgressCompletion::Cancelled {
            count: 1,
            bytes: 1024,
        },
        crate::CleanupProgressCompletion::Failed {
            message: "Docker cleanup failed: daemon unavailable".into(),
        },
    ];
    for comp in completions {
        if let Ok(mut snap) = progress_handle.lock() {
            snap.completion = Some(comp);
        }
        for locale in crate::i18n::ALL_LOCALES {
            host_win.update(host_cx, |host, cx| {
                host.inner.update(cx, |win, cx| {
                    win.set_locale(locale);
                    cx.notify();
                });
            });
            draw(host_cx);
            assert!(
                host_cx.debug_bounds("cleanup-progress-done-icon").is_some()
            );
            assert!(host_cx.debug_bounds("cleanup-progress-spinner").is_none());
            assert!(host_cx.debug_bounds("cleanup-progress-cancel").is_none());
            assert!(
                host_cx
                    .debug_bounds("cleanup-progress-background")
                    .is_none()
            );
            let close_b = host_cx
                .debug_bounds("cleanup-progress-close")
                .expect("close button at min size");
            assert!(
                close_b.bottom() <= px(280.0),
                "close button overflowed 460x280 in {locale:?}: {close_b:?}"
            );
        }
    }
    drop(host_win);

    // 2. Manual early click on `cleanup-progress-close` closes immediately and
    //    advancing the clock past 3s produces no error or stale close.
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let early_file = temp.path().join("junk/early_close.bin");
    std::fs::write(&early_file, vec![b'e'; 512]).expect("write early file");
    let proposal_a = CleanupProposal {
        timestamp_secs: 1_700_000_200,
        summary: "Early close test".into(),
        items: vec![ProposalItem::new(
            early_file.clone(),
            512,
            RemovalMode::Permanent,
            None,
            None,
        )],
        docker_targets: Vec::new(),
    };
    {
        let db = StorageDb::open(&db_path).expect("open db");
        let st =
            AiState::from_proposal(&proposal_a, 1_700_000_200).expect("st");
        db.save_ai_state(&st).expect("save st");
    }
    cx.update(|cx| {
        crate::install_notification_handler(cx);
        crate::install_test_runtime(temp.path().to_path_buf(), false, None, cx);
    });
    let req_a = crate::state::proposal_confirm_request(
        temp.path().to_path_buf(),
        &proposal_a,
        crate::i18n::Locale::En,
    );
    let confirm_a = cx
        .update(|cx| {
            crate::open_local_confirm_window(
                req_a,
                Some(db_path.clone()),
                ruum_core::docker_adapter::DockerProbeStatus::NotInstalled,
                cx,
            )
        })
        .expect("confirm win a");
    let mut confirm_a_cx = VisualTestContext::from_window(confirm_a.into(), cx);
    draw(&mut confirm_a_cx);
    let confirm_btn_a = confirm_a_cx
        .debug_bounds("confirm-clean")
        .expect("confirm btn a");
    confirm_a_cx.simulate_click(confirm_btn_a.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(!early_file.exists());

    let prog_a = cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.active_cleanup.is_none());
        rt.cleanup_progress_window
            .expect("lingering window before manual close")
    });
    let mut prog_a_cx = VisualTestContext::from_window(prog_a.into(), cx);
    draw(&mut prog_a_cx);
    let close_btn = prog_a_cx
        .debug_bounds("cleanup-progress-close")
        .expect("cleanup-progress-close");
    prog_a_cx.simulate_click(close_btn.center(), Modifiers::none());
    cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.cleanup_progress_window.is_none());
        assert!(!cx.windows().contains(&prog_a.into()));
    });
    cx.background_executor
        .advance_clock(crate::CLEANUP_PROGRESS_LINGER + Duration::from_secs(1));
    cx.run_until_parked();
    cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.cleanup_progress_window.is_none());
    });

    // 3. Clicking `Continue in Background` before completion keeps
    //    `cleanup_progress_window` as `None` when cleanup finishes (no reopen).
    let bg_file = temp.path().join("junk/bg_finish.bin");
    std::fs::write(&bg_file, vec![b'b'; 512]).expect("write bg file");
    let proposal_b = CleanupProposal {
        timestamp_secs: 1_700_000_300,
        summary: "Background finish test".into(),
        items: vec![ProposalItem::new(
            bg_file.clone(),
            512,
            RemovalMode::Permanent,
            None,
            None,
        )],
        docker_targets: Vec::new(),
    };
    {
        let db = StorageDb::open(&db_path).expect("open db");
        let st =
            AiState::from_proposal(&proposal_b, 1_700_000_300).expect("st");
        db.save_ai_state(&st).expect("save st");
    }
    let cleanup_gate = Arc::new(crate::AsyncSignal::default());
    let cleanup_gate_rt = Arc::clone(&cleanup_gate);
    cx.update(|cx| {
        crate::with_runtime_mut(cx, |rt| {
            rt.test_cleanup_gate = Some(cleanup_gate_rt);
        });
    });
    let req_b = crate::state::proposal_confirm_request(
        temp.path().to_path_buf(),
        &proposal_b,
        crate::i18n::Locale::En,
    );
    let confirm_b = cx
        .update(|cx| {
            crate::open_local_confirm_window(
                req_b,
                Some(db_path),
                ruum_core::docker_adapter::DockerProbeStatus::NotInstalled,
                cx,
            )
        })
        .expect("confirm win b");
    let mut confirm_b_cx = VisualTestContext::from_window(confirm_b.into(), cx);
    draw(&mut confirm_b_cx);
    let confirm_btn_b = confirm_b_cx
        .debug_bounds("confirm-clean")
        .expect("confirm btn b");
    confirm_b_cx.simulate_click(confirm_btn_b.center(), Modifiers::none());

    let prog_b = cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.active_cleanup.is_some());
        rt.cleanup_progress_window
            .expect("progress window open before background")
    });
    let mut prog_b_cx = VisualTestContext::from_window(prog_b.into(), cx);
    draw(&mut prog_b_cx);
    let bg_btn = prog_b_cx
        .debug_bounds("cleanup-progress-background")
        .expect("background button");
    prog_b_cx.simulate_click(bg_btn.center(), Modifiers::none());
    cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.cleanup_progress_window.is_none());
        assert!(rt.active_cleanup.is_some());
    });

    cleanup_gate.notify();
    cx.run_until_parked();
    assert!(!bg_file.exists());
    cx.read(|cx| {
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.active_cleanup.is_none());
        assert!(rt.cleanup_progress_window.is_none());
    });
}

#[gpui_kit::test]
#[allow(
    clippy::needless_pass_by_ref_mut,
    reason = "gpui_kit::test passes &mut TestAppContext"
)]
fn request_quit_cancels_active_cleanup_and_closes_progress_window(
    cx: &mut TestAppContext,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    cx.update(gpui_omarchy::init);
    let temp = tempfile::tempdir().expect("tempdir");
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_bg = Arc::clone(&cancel);
    let progress = Arc::new(Mutex::new(crate::CleanupProgressSnapshot {
        total_items: 5,
        total_bytes: 5000,
        completed_items: 1,
        reclaimed_bytes: 1000,
        current_item: Some("~/junk/item.bin".into()),
        cancelling: false,
        completion: None,
    }));

    cx.update(|cx| {
        crate::install_test_runtime(temp.path().to_path_buf(), false, None, cx);
        crate::with_runtime_mut(cx, |rt| {
            rt.active_cleanup = Some(crate::ActiveCleanupTask {
                cancel: Arc::clone(&cancel),
                progress,
            });
        });
        let win = crate::open_cleanup_progress_window(cx)
            .expect("open progress window");
        assert!(cx.windows().contains(&win.into()));
        crate::app_menu::request_quit(cx);
        assert!(crate::app_menu::is_quitting(cx));
        let rt = cx.global::<crate::AppRuntime>();
        assert!(rt.cleanup_progress_window.is_none());
        assert!(rt.active_cleanup.is_none());
    });
    assert!(cancel_bg.load(Ordering::Acquire));
}

#[gpui_kit::test]
fn explore_top_bar_ai_button_layout_icons_and_locales(cx: &mut TestAppContext) {
    use gpui_kit::AssetSource as _;
    use gpui_kit::assets::IconName;

    assert!(
        crate::RuumAssets
            .load(IconName::Bot.path().as_ref())
            .expect("bot icon load")
            .is_some()
    );

    cx.update(gpui_omarchy::init);
    let temp = fixture();
    let (view, cx) = view_over(temp.path(), cx);

    let assert_inside =
        |name: &str, bounds: Bounds<Pixels>, width: f32, height: f32| {
            assert!(
                bounds.origin.x >= px(0.)
                    && bounds.right() <= px(width)
                    && bounds.origin.y >= px(0.)
                    && bounds.bottom() <= px(height),
                "{name} out of {width}x{height} bounds: {bounds:?}"
            );
        };

    for (width, height) in [(1440., 900.), (900., 600.)] {
        cx.simulate_resize(gpui_kit::size(px(width), px(height)));
        for locale in crate::i18n::ALL_LOCALES {
            update(&view, cx, |app, cx| {
                app.set_locale(locale);
                app.watchdog_analyzing = false;
                cx.notify();
            });
            draw(cx);
            let switch_b = cx
                .debug_bounds("trail-switch-root")
                .expect("trail-switch-root");
            let ai_b = cx
                .debug_bounds("trail-ai-analyze")
                .expect("trail-ai-analyze");
            assert_inside("trail-switch-root", switch_b, width, height);
            assert_inside("trail-ai-analyze", ai_b, width, height);
            assert!(
                ai_b.left() >= switch_b.right(),
                "expected trail-ai-analyze beside trail-switch-root in \
                 {locale:?}: switch={switch_b:?}, ai={ai_b:?}"
            );
            assert!(
                (ai_b.center().y - switch_b.center().y).abs() < px(10.),
                "top bar buttons misaligned in {locale:?}: \
                 switch={switch_b:?}, ai={ai_b:?}"
            );
            assert!(cx.debug_bounds("trail-ai-analyze-spin").is_none());
        }
    }

    update(&view, cx, |app, cx| {
        app.watchdog_analyzing = true;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("trail-ai-analyze").is_some());
    assert!(cx.debug_bounds("trail-ai-analyze-spin").is_some());
}

#[gpui_kit::test]
fn explore_ai_button_local_click_dispatches_and_guards_reentry(
    cx: &mut TestAppContext,
) {
    use gpui_kit::Modifiers;
    use ruum_core::watchdog_config::AiCliProvider;
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    cx.update(gpui_omarchy::init);
    cx.update(|cx| crate::install_notification_handler(cx));
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let enc_path = cfg_dir.path().join("creds.enc");

    let counter = CountingRootSecret::new([0x61; 32]);
    let store = counter.store_for(&enc_path);
    store.set_ai_api_key("test-key").expect("save api key");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock api");
    let addr = listener.local_addr().expect("mock addr");
    let req_count = Arc::new(AtomicUsize::new(0));
    let req_count_srv = Arc::clone(&req_count);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_srv = Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        while !stop_srv.load(Ordering::Acquire) {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            if stop_srv.load(Ordering::Acquire) {
                break;
            }
            let _ = read_test_http_request(&mut stream);
            req_count_srv.fetch_add(1, Ordering::AcqRel);
            let proposal_json = serde_json::json!({
                "summary": "All clean",
                "items": []
            })
            .to_string();
            let chunk1 = serde_json::json!({
                "id": "c-local",
                "object": "chat.completion.chunk",
                "model": "gpt-test",
                "choices": [{
                    "index": 0,
                    "delta": { "role": "assistant", "content": proposal_json },
                    "finish_reason": null
                }]
            });
            let chunk2 = serde_json::json!({
                "id": "c-local",
                "object": "chat.completion.chunk",
                "model": "gpt-test",
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop"
                }]
            });
            let sse =
                format!("data: {chunk1}\n\ndata: {chunk2}\n\ndata: [DONE]\n\n");
            write_test_http_response(
                &mut stream,
                "200 OK",
                "text/event-stream",
                &sse,
            );
        }
    });

    let (view, win_cx) = view_over(temp.path(), cx);
    let home_root =
        std::env::home_dir().unwrap_or_else(|| temp.path().to_path_buf());
    update(&view, win_cx, |app, cx| {
        app.root_path = home_root;
        app.db_path = Some(db_path);
        app.cred_store = Some(store);
        app.set_locale(crate::i18n::Locale::En);
        app.ai_config.provider = AiCliProvider::Api;
        app.watchdog_draft.ai_provider = AiCliProvider::Api;
        app.ai_config.api_base_url = format!("http://{addr}/v1");
        app.ai_config.model = Some("gpt-test".into());
        app.watchdog_draft.model = Some("gpt-test".into());
        app.watchdog_input_api_base_url = None;
        app.watchdog_input_model = None;
        cx.notify();
    });
    draw(win_cx);
    assert!(win_cx.debug_bounds("trail-ai-analyze-spin").is_none());

    // 1. Clicking `trail-ai-analyze` on HostTabKey::Local dispatches
    //    `run_ai_analysis_now` and settles on Explore.
    let ai_bounds = win_cx
        .debug_bounds("trail-ai-analyze")
        .expect("trail-ai-analyze");
    win_cx.simulate_click(ai_bounds.center(), Modifiers::none());
    win_cx.run_until_parked();
    read(&view, win_cx, |app| {
        assert_eq!(app.screen, Screen::Explore);
        assert!(!app.watchdog_analyzing);
        let expected = crate::i18n::messages::ai_found_no_files(app.locale);
        assert_eq!(
            app.notice.as_ref().map(|(m, _)| m.as_str()),
            Some(expected)
        );
    });
    draw(win_cx);
    assert!(win_cx.debug_bounds("trail-ai-analyze-spin").is_none());
    assert_eq!(req_count.load(Ordering::Acquire), 1);

    // 2. While `watchdog_analyzing` is true, `trail-ai-analyze-spin` renders
    //    and clicking `trail-ai-analyze` is guarded against re-entry.
    update(&view, win_cx, |app, cx| {
        app.notice = None;
        app.run_ai_analysis_now(cx);
    });
    draw(win_cx);
    let in_flight_epoch = read(&view, win_cx, |app| {
        assert!(app.watchdog_analyzing);
        app.watchdog_analysis_epoch
    });
    assert!(win_cx.debug_bounds("trail-ai-analyze-spin").is_some());

    win_cx.simulate_click(ai_bounds.center(), Modifiers::none());
    win_cx.run_until_parked();

    stop.store(true, Ordering::Release);
    let _ = TcpStream::connect(addr);
    server.join().expect("server join");

    read(&view, win_cx, |app| {
        assert!(!app.watchdog_analyzing);
        assert_eq!(app.watchdog_analysis_epoch, in_flight_epoch);
    });
    draw(win_cx);
    assert!(win_cx.debug_bounds("trail-ai-analyze-spin").is_none());
    assert_eq!(req_count.load(Ordering::Acquire), 2);
}

#[gpui_kit::test]
fn explore_ai_button_ssh_tab_dispatches_and_respects_state(
    cx: &mut TestAppContext,
) {
    use crate::state::{ExploreHostTab, ExploreSessionSnapshot, HostTabKey};
    use gpui_kit::Modifiers;
    use ruum_core::credentials::SshCredentialSecret;
    use ruum_core::ssh_session::{SshAuthMaterial, SshEndpoint, SshSession};
    use ruum_core::storage::{AuthMode, SshProfile};
    use std::net::TcpListener;
    use std::sync::Arc;

    cx.update(gpui_omarchy::init);
    cx.update(|cx| crate::install_notification_handler(cx));
    let temp = fixture();
    let cfg_dir = tempfile::tempdir().expect("cfg dir");
    let db_path = cfg_dir.path().join("ruum.db");
    let enc_path = cfg_dir.path().join("creds.enc");
    let counter = CountingRootSecret::new([0x62; 32]);

    // Closed ephemeral port ensures instant ECONNREFUSED if SSH runs.
    let closed_port = TcpListener::bind("127.0.0.1:0")
        .expect("bind port")
        .local_addr()
        .expect("addr")
        .port();

    let profile = SshProfile {
        id: "srv-explore-ai".into(),
        alias: "prod-ai".into(),
        host: "127.0.0.1".into(),
        port: closed_port,
        username: "deploy".into(),
        auth_mode: AuthMode::Password,
        default_path: Some("~".into()),
        created_at_secs: 1,
        updated_at_secs: 1,
        last_total_bytes: None,
        last_available_bytes: None,
        last_scanned_secs: None,
        pending_script_path: None,
        pending_script_items: None,
        pending_reclaim_bytes: None,
        pending_created_secs: None,
    };

    let (view, win_cx) = view_over(temp.path(), cx);
    let mut remote_snap = ExploreSessionSnapshot::default();
    remote_snap.root_path = PathBuf::from("/home/deploy");
    remote_snap.scan_root = PathBuf::from("/home/deploy");
    remote_snap.tree =
        Some(Arc::new(scan(temp.path(), options()).expect("scan")));
    remote_snap.remote = Some("prod-ai".into());
    remote_snap.remote_platform = "linux".into();
    remote_snap.active_ssh_profile_id = Some(profile.id.clone());

    update(&view, win_cx, |app, cx| {
        app.db_path = Some(db_path);
        app.cred_store = Some(counter.store_for(&enc_path));
        app.ssh_profiles = vec![profile.clone()];
        app.host_tabs.push(ExploreHostTab {
            key: HostTabKey::Ssh(profile.id.clone()),
            label: "prod-ai".into(),
            session: Some(remote_snap),
        });
        app.switch_host_tab(HostTabKey::Ssh(profile.id.clone()), cx);
    });
    draw(win_cx);

    // 1. Disconnected SSH tab (`active_ssh_session == None`): button is
    //    disabled and click does not dispatch analysis.
    let ai_bounds = win_cx
        .debug_bounds("trail-ai-analyze")
        .expect("trail-ai-analyze");
    assert!(win_cx.debug_bounds("trail-ai-analyze-spin").is_none());
    win_cx.simulate_click(ai_bounds.center(), Modifiers::none());
    draw(win_cx);
    read(&view, win_cx, |app| {
        assert!(!app.ssh_analyzing_profiles.contains("srv-explore-ai"));
        assert!(app.notice.is_none());
    });

    // 2. Closed SSH session is also treated as disconnected.
    let endpoint = SshEndpoint::from_profile(&profile).expect("endpoint");
    let material = SshAuthMaterial::from_credential_secret(
        &SshCredentialSecret::Password {
            password: "dummy-password".into(),
        },
    );
    let mut closed_session =
        SshSession::start(endpoint.clone(), material.clone(), 15)
            .expect("start closed session");
    closed_session.close();
    update(&view, win_cx, |app, cx| {
        app.active_ssh_session = Some(Arc::new(closed_session));
        cx.notify();
    });
    draw(win_cx);
    win_cx.simulate_click(ai_bounds.center(), Modifiers::none());
    draw(win_cx);
    read(&view, win_cx, |app| {
        assert!(!app.ssh_analyzing_profiles.contains("srv-explore-ai"));
        assert!(app.notice.is_none());
    });

    // 3. Live SSH session shows spinner while in-flight and ignores clicks.
    let live_session =
        Arc::new(SshSession::start(endpoint, material, 15).expect("live"));
    update(&view, win_cx, |app, cx| {
        app.active_ssh_session = Some(live_session);
        app.ssh_analyzing_profiles.insert("srv-explore-ai".into());
        cx.notify();
    });
    draw(win_cx);
    assert!(win_cx.debug_bounds("trail-ai-analyze-spin").is_some());
    win_cx.simulate_click(ai_bounds.center(), Modifiers::none());
    draw(win_cx);
    read(&view, win_cx, |app| {
        assert!(app.notice.is_none());
    });

    // 4. Connected idle SSH tab dispatches `analyze_ssh_profile(&id, cx)`.
    update(&view, win_cx, |app, cx| {
        app.ssh_analyzing_profiles.remove("srv-explore-ai");
        cx.notify();
    });
    draw(win_cx);
    assert!(win_cx.debug_bounds("trail-ai-analyze-spin").is_none());

    win_cx.simulate_click(ai_bounds.center(), Modifiers::none());
    win_cx.run_until_parked();
    read(&view, win_cx, |app| {
        assert!(!app.ssh_analyzing_profiles.contains("srv-explore-ai"));
        let (msg, status) = app.notice.as_ref().expect("ssh analyze notice");
        assert_eq!(*status, gpui_omarchy::Status::Error);
        assert!(msg.contains("prod-ai"), "unexpected notice: {msg}");
    });
}
