use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use bevy::camera::visibility::RenderLayers;
use bevy::color::palettes::tailwind;
use bevy::light::NotShadowCaster;
use bevy::gltf::Gltf;
use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiContexts, egui};
use bevy_enhanced_input::prelude::*;
use bevy_kira_audio::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use multiplayer::player::*;
use multiplayer::protocol::*;
use multiplayer::world::{
    spawn_lights, spawn_world_model, update_view_model, WorldModelCamera, DEFAULT_RENDER_LAYER,
    interaction_ui_system, init_replicated_doors, init_replicated_equippables,
    init_replicated_interactables, sync_door_state, sync_equippable_position, sync_equippable_visibility,
    sync_remote_equipped, spawn_tracer, cleanup_tracers, remote_shot_tracers,
    start_jab_animation, animate_jab, LeftHand,
};
use multiplayer::{SharedPlugin, FIXED_TIMESTEP_HZ, PROTOCOL_ID, SERVER_PORT};

// ========================================
// App State
// ========================================

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
enum AppState {
    #[default]
    Loading,
    MainMenu,
    InGame,
}

/// Tracks GLTF asset loading.
#[derive(Resource)]
struct AssetLoadTracker {
    handles: Vec<Handle<Gltf>>,
}

/// Marker for the menu Camera2d — despawned when entering InGame.
#[derive(Component)]
struct MenuCamera;

/// Marker: egui fonts have been configured.
#[derive(Resource)]
struct EguiFontsReady;

/// Handle to the menu music instance — stopped when entering InGame.
#[derive(Resource)]
struct MenuMusicHandle(Handle<AudioInstance>);

/// Anima cover image handle — loaded during asset loading, displayed on menu.
#[derive(Resource)]
struct AnimaCover(Handle<Image>);

/// Line gradient image for accent divider.
#[derive(Resource)]
struct LineGradient(Handle<Image>);

/// Tracks which menu item is selected (for keyboard navigation).
#[derive(Resource, Default)]
struct MenuSelection(usize);

fn main() {
    eprintln!(
        "Anima Client {} (commit {} built {})",
        env!("ANIMA_VERSION"),
        env!("ANIMA_BUILD_SHA"),
        env!("ANIMA_BUILD_DATE"),
    );

    // Load or generate persistent Ed25519 keypair (~/.anima/keypair.json)
    let identity = multiplayer::auth::ClientIdentity::load_or_create();
    info!("Client identity: {} (id={})", identity.address, identity.client_id);

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(bevy::log::LogPlugin {
        filter: "bevy_enhanced_input::action::fns=error".into(),
        ..default()
    }).set(WindowPlugin {
        primary_window: Some(Window {
            title: format!("ANIMA {} — {}", env!("ANIMA_VERSION"), &identity.address[..8]),
            ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(Color::BLACK));
    app.insert_resource(identity);
    app.add_plugins(EguiPlugin::default());
    app.add_plugins(AudioPlugin);
    app.add_plugins(ClientPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
    });
    app.add_plugins(SharedPlugin);
    app.init_state::<AppState>();
    app.insert_resource(CursorState::default());
    // One Camera2d in Startup — persists until InGame
    app.add_systems(Startup, setup);

    // Font setup — runs until fonts are loaded
    app.add_systems(Update, setup_egui_fonts.run_if(not(resource_exists::<EguiFontsReady>)));

    // Loading
    app.add_systems(OnEnter(AppState::Loading), loading_setup);
    app.add_systems(Update, (loading_check, loading_ui).run_if(in_state(AppState::Loading)));

    // MainMenu
    app.add_systems(OnEnter(AppState::MainMenu), menu_enter);
    app.add_systems(Update, menu_ui.run_if(in_state(AppState::MainMenu)));

    // InGame
    app.add_systems(
        OnEnter(AppState::InGame),
        (despawn_menu, spawn_world_model, spawn_lights, connect_to_server),
    );
    app.add_systems(
        Update,
        (
            sync_camera_pitch,
            grab_mouse,
            change_fov,
            update_view_model,
            interaction_ui_system,
            sync_door_state,
            init_replicated_doors,
            init_replicated_equippables,
            init_replicated_interactables,
        )
            .run_if(in_state(AppState::InGame)),
    );
    app.add_systems(
        Update,
        (sync_equippable_visibility, sync_equippable_position, sync_remote_equipped)
            .run_if(in_state(AppState::InGame))
            .run_if(not(lightyear::prelude::is_in_rollback)),
    );
    app.add_systems(
        FixedPreUpdate,
        (pre_rotate_move_input, gate_look_on_cursor)
            .after(EnhancedInputSystems::Update)
            .before(lightyear::prelude::client::input::InputSystems::BufferClientInputs)
            .run_if(not(lightyear::prelude::is_in_rollback))
            .run_if(in_state(AppState::InGame)),
    );

    app.add_systems(
        Update,
        (cleanup_tracers, remote_shot_tracers, animate_jab, crosshair_hud, health_hud, inventory_hud, death_screen, kill_feed_ui, build_version_hud, log_health_changes)
            .run_if(in_state(AppState::InGame)),
    );

    // Wallet auth: send signed proof to server after connection established
    app.add_systems(
        Update,
        send_wallet_auth.run_if(in_state(AppState::InGame)),
    );

    app.add_observer(on_predicted_spawn);
    app.add_observer(on_interpolated_spawn);
    app.add_observer(spawn_tracer);
    app.add_observer(start_jab_animation);
    app.run();
}

// ========================================
// Setup
// ========================================

fn setup(mut commands: Commands) {
    commands.spawn((
        MenuCamera,
        Camera2d,
        Camera {
            order: 10,
            clear_color: ClearColorConfig::None,
            ..default()
        },
    ));
}

/// Load custom fonts into egui. Runs every frame until the egui context is available.
fn setup_egui_fonts(mut contexts: EguiContexts, mut commands: Commands) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    let mut fonts = egui::FontDefinitions::default();

    // Cinzel Regular — title "I Always"
    fonts.font_data.insert(
        "cinzel".into(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/Cinzel/static/Cinzel-Regular.ttf")).into(),
    );

    // Cinzel Bold
    fonts.font_data.insert(
        "cinzel_bold".into(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/Cinzel/static/Cinzel-Bold.ttf")).into(),
    );

    // Cinzel Black — max weight, for title
    fonts.font_data.insert(
        "cinzel_black".into(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/Cinzel/static/Cinzel-Black.ttf")).into(),
    );

    // Chakra Petch Regular — body/UI text
    fonts.font_data.insert(
        "chakra".into(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/Chakra_Petch/ChakraPetch-Regular.ttf")).into(),
    );

    // Chakra Petch SemiBold — emphasized UI text
    fonts.font_data.insert(
        "chakra_semi".into(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/Chakra_Petch/ChakraPetch-SemiBold.ttf")).into(),
    );

    // Chakra Petch Bold — menu items
    fonts.font_data.insert(
        "chakra_bold".into(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/Chakra_Petch/ChakraPetch-Bold.ttf")).into(),
    );

    // Named font families
    fonts.families.insert(
        egui::FontFamily::Name("cinzel".into()),
        vec!["cinzel".into()],
    );
    fonts.families.insert(
        egui::FontFamily::Name("cinzel_bold".into()),
        vec!["cinzel_bold".into()],
    );
    fonts.families.insert(
        egui::FontFamily::Name("cinzel_black".into()),
        vec!["cinzel_black".into()],
    );
    fonts.families.insert(
        egui::FontFamily::Name("chakra".into()),
        vec!["chakra".into()],
    );
    fonts.families.insert(
        egui::FontFamily::Name("chakra_semi".into()),
        vec!["chakra_semi".into()],
    );
    fonts.families.insert(
        egui::FontFamily::Name("chakra_bold".into()),
        vec!["chakra_bold".into()],
    );

    // Set Chakra Petch as default proportional font
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "chakra".into());

    ctx.set_fonts(fonts);
    commands.insert_resource(EguiFontsReady);
    info!("Custom fonts loaded into egui");
}

// ========================================
// Shared UI helpers
// ========================================

/// Cinzel font ID at the given size (regular weight).
fn cinzel(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("cinzel".into()))
}

/// Cinzel Bold font ID at the given size.
fn cinzel_bold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("cinzel_bold".into()))
}

/// Cinzel Black (max weight) font ID at the given size.
fn cinzel_black(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("cinzel_black".into()))
}

/// Chakra Petch font ID at the given size.
fn chakra(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("chakra".into()))
}

/// Chakra Petch SemiBold font ID at the given size.
fn chakra_semi(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("chakra_semi".into()))
}

/// Chakra Petch Bold font ID at the given size.
fn chakra_bold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("chakra_bold".into()))
}

// Color palette — cream #F0E8D8 and blue rgba(100, 160, 255) at various opacities
const GEO_COLOR_BASE: [u8; 3] = [60, 70, 120];

/// Cream #F0E8D8 at a given alpha (0.0–1.0).
fn cream(alpha: f32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(240, 232, 216, (alpha * 255.0) as u8)
}

/// Blue accent rgba(100, 160, 255) at a given alpha (0.0–1.0).
fn blue(alpha: f32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(100, 160, 255, (alpha * 255.0) as u8)
}

/// Draw text with custom letter-spacing (in em units). Returns total width.
fn draw_spaced_text(
    painter: &egui::Painter,
    pos: egui::Pos2,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    letter_spacing_em: f32,
) -> f32 {
    let spacing = font.size * letter_spacing_em;
    let mut x = pos.x;
    for ch in text.chars() {
        let galley = painter.layout_no_wrap(ch.to_string(), font.clone(), color);
        let char_width = galley.size().x;
        painter.galley(egui::pos2(x, pos.y), galley, color);
        x += char_width + spacing;
    }
    x - pos.x - spacing // total width (subtract trailing spacing)
}

/// Draw animated geometric shapes behind the menu content (centered on screen).
fn draw_geometric_background(ui: &egui::Ui, t: f32) {
    let painter = ui.painter();
    let rect = ui.max_rect();
    let center = rect.center();
    draw_geometric_background_at(painter, rect, center, t);
}

/// Draw animated geometric shapes at a specific center point.
fn draw_geometric_background_at(painter: &egui::Painter, rect: egui::Rect, center: egui::Pos2, t: f32) {

    // Slowly rotating thin lines radiating from center
    for i in 0..8 {
        let angle = t * 0.05 + (i as f32) * std::f32::consts::TAU / 8.0;
        let len = rect.width().min(rect.height()) * 0.45;
        let inner = 60.0 + ((t * 0.3 + i as f32 * 0.5).sin() * 20.0);
        let start = center + egui::vec2(angle.cos() * inner, angle.sin() * inner);
        let end = center + egui::vec2(angle.cos() * len, angle.sin() * len);
        let alpha = ((t * 0.4 + i as f32 * 0.8).sin() * 0.5 + 0.5) * 20.0;
        painter.line_segment(
            [start, end],
            egui::Stroke::new(
                0.5,
                egui::Color32::from_rgba_unmultiplied(GEO_COLOR_BASE[0], GEO_COLOR_BASE[1], GEO_COLOR_BASE[2], alpha as u8),
            ),
        );
    }

    // Concentric pulsing circles
    for i in 0..4 {
        let base_radius = 80.0 + i as f32 * 100.0;
        let radius = base_radius + (t * 0.2 + i as f32 * 0.9).sin() * 15.0;
        let alpha = ((t * 0.15 + i as f32 * 0.6).sin() * 0.5 + 0.5) * 18.0;
        painter.circle_stroke(
            center,
            radius,
            egui::Stroke::new(
                0.5,
                egui::Color32::from_rgba_unmultiplied(GEO_COLOR_BASE[0], GEO_COLOR_BASE[1], GEO_COLOR_BASE[2], alpha as u8),
            ),
        );
    }

    // Slow horizontal scan line
    let scan_y = rect.top() + ((t * 0.08).sin() * 0.5 + 0.5) * rect.height();
    painter.line_segment(
        [
            egui::pos2(rect.left(), scan_y),
            egui::pos2(rect.right(), scan_y),
        ],
        egui::Stroke::new(
            0.3,
            egui::Color32::from_rgba_unmultiplied(80, 90, 140, 12),
        ),
    );

    // Corner accent marks — small geometric brackets
    let corner_len = 30.0;
    let margin = 40.0;
    let corner_alpha = ((t * 0.3).sin() * 0.5 + 0.5) * 35.0;
    let corner_color = egui::Color32::from_rgba_unmultiplied(GEO_COLOR_BASE[0], GEO_COLOR_BASE[1], GEO_COLOR_BASE[2], corner_alpha as u8);
    let stroke = egui::Stroke::new(1.0, corner_color);

    // Top-left
    painter.line_segment([egui::pos2(rect.left() + margin, rect.top() + margin), egui::pos2(rect.left() + margin + corner_len, rect.top() + margin)], stroke);
    painter.line_segment([egui::pos2(rect.left() + margin, rect.top() + margin), egui::pos2(rect.left() + margin, rect.top() + margin + corner_len)], stroke);
    // Top-right
    painter.line_segment([egui::pos2(rect.right() - margin, rect.top() + margin), egui::pos2(rect.right() - margin - corner_len, rect.top() + margin)], stroke);
    painter.line_segment([egui::pos2(rect.right() - margin, rect.top() + margin), egui::pos2(rect.right() - margin, rect.top() + margin + corner_len)], stroke);
    // Bottom-left
    painter.line_segment([egui::pos2(rect.left() + margin, rect.bottom() - margin), egui::pos2(rect.left() + margin + corner_len, rect.bottom() - margin)], stroke);
    painter.line_segment([egui::pos2(rect.left() + margin, rect.bottom() - margin), egui::pos2(rect.left() + margin, rect.bottom() - margin - corner_len)], stroke);
    // Bottom-right
    painter.line_segment([egui::pos2(rect.right() - margin, rect.bottom() - margin), egui::pos2(rect.right() - margin - corner_len, rect.bottom() - margin)], stroke);
    painter.line_segment([egui::pos2(rect.right() - margin, rect.bottom() - margin), egui::pos2(rect.right() - margin, rect.bottom() - margin - corner_len)], stroke);
}

// ========================================
// Loading state
// ========================================

fn loading_setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    // Don't preload raw .glb files — Bevy auto-spawns their default scenes at origin.
    // Models are loaded on demand by init_replicated_equippables/interactables via Scene(0).
    let handles: Vec<Handle<Gltf>> = vec![];
    commands.insert_resource(AssetLoadTracker { handles });

    // Preload the Anima cover image for the menu
    let cover: Handle<Image> = asset_server.load("images/anima-cover.png");
    commands.insert_resource(AnimaCover(cover));

    let line_grad: Handle<Image> = asset_server.load("images/line-gradient.png");
    commands.insert_resource(LineGradient(line_grad));

    info!("Loading assets...");
}

fn loading_ui(mut contexts: EguiContexts, time: Res<Time>, mut frame_count: Local<u32>) {
    *frame_count += 1;
    if *frame_count <= 2 { return; } // egui context not ready on first frames
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    let t = time.elapsed_secs();

    // Request repaint for animations
    ctx.request_repaint();

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
        .show(ctx, |ui| {
            draw_geometric_background(ui, t);

            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                let space = (ui.available_height() - 160.0).max(0.0) / 2.0;
                ui.add_space(space);

                ui.label(
                    egui::RichText::new("ANIMA")
                        .font(cinzel_bold(64.0))
                        .color(cream(1.0)),
                );

                ui.add_space(32.0);

                let dots = match ((t * 2.0) as u32) % 4 {
                    0 => ".",
                    1 => ". .",
                    2 => ". . .",
                    _ => "",
                };
                ui.label(
                    egui::RichText::new(format!("L O A D I N G {}", dots))
                        .font(chakra(16.0))
                        .color(cream(0.4)),
                );
            });
        });
}

fn loading_check(
    mut commands: Commands,
    tracker: Option<Res<AssetLoadTracker>>,
    mut next_state: ResMut<NextState<AppState>>,
    asset_server: Res<AssetServer>,
) {
    let Some(tracker) = tracker else { return; };
    let all_loaded = tracker.handles.iter().all(|h| {
        matches!(asset_server.get_load_state(h), Some(bevy::asset::LoadState::Loaded))
    });
    if !all_loaded { return; }

    info!("Assets loaded");
    next_state.set(AppState::MainMenu);
    commands.remove_resource::<AssetLoadTracker>();
}

// ========================================
// Main menu state
// ========================================

fn menu_enter(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    audio: Res<Audio>,
) {
    let music = asset_server.load("audio/menu.mp3");
    let handle = audio.play(music).looped().with_volume(0.5).handle();
    commands.insert_resource(MenuMusicHandle(handle));
    commands.insert_resource(MenuSelection::default());
    info!("Main menu entered — music playing");
}

fn menu_ui(
    mut contexts: EguiContexts,
    keys: Res<ButtonInput<KeyCode>>,
    mut next_state: ResMut<NextState<AppState>>,
    anima_cover: Option<Res<AnimaCover>>,
    line_gradient: Option<Res<LineGradient>>,
    mut menu_sel: ResMut<MenuSelection>,
    mut frame_count: Local<u32>,
) {
    *frame_count += 1;
    if *frame_count <= 2 { return; }

    // Register images with egui (must happen before ctx_mut borrow)
    let cover_tex = anima_cover.as_ref().map(|c| {
        contexts.add_image(bevy_egui::EguiTextureHandle::Strong(c.0.clone()))
    });
    let line_tex = line_gradient.as_ref().map(|l| {
        contexts.add_image(bevy_egui::EguiTextureHandle::Strong(l.0.clone()))
    });

    let Ok(ctx) = contexts.ctx_mut() else { return; };

    let mut menu_anchor_x = 0.0_f32;
    let mut menu_anchor_y = 0.0_f32;

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
        .show(ctx, |ui| {
            let rect = ui.max_rect();

            // --- Anima cover image — left-aligned, scaled to screen height ---
            let img_aspect = 1024.0 / 1536.0;
            let img_h = rect.height();
            let img_w = img_h * img_aspect;

            if let Some(tex_id) = cover_tex {
                let img_rect = egui::Rect::from_min_size(
                    rect.left_top(),
                    egui::vec2(img_w, img_h),
                );
                ui.painter().image(
                    tex_id,
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }

            // --- Version text — bottom right, cream at 0.2 ---
            ui.painter().text(
                egui::pos2(rect.right() - 20.0, rect.bottom() - 20.0),
                egui::Align2::RIGHT_BOTTOM,
                &format!("v{}-{}", env!("CARGO_PKG_VERSION"), env!("GIT_SHORT_HASH")),
                chakra(11.0),
                cream(0.2),
            );

            // --- Right side content — anchored to right half of screen ---
            let half = rect.center().x;
            let padding = (rect.width() * 0.04).max(32.0);
            let content_x = half + padding;

            // Vertically center the content block
            let content_y = rect.top() + (rect.height() - 400.0).max(0.0) * 0.38;

            // Title block — painted directly
            let painter = ui.painter();
            let mut y = content_y;

            // "PROJECT CODENAME"
            let galley = painter.layout_no_wrap(
                "PROJECT CODENAME".into(), chakra(12.0), blue(0.7),
            );
            draw_spaced_text(painter, egui::pos2(content_x, y), "PROJECT CODENAME", chakra(12.0), blue(0.7), 0.15);
            y += galley.size().y + 8.0;

            // "ANIMA" title with glow
            let title_font = cinzel_black(72.0);
            for dx in [-2.0, 0.0, 2.0_f32] {
                for dy in [-1.0, 0.0, 1.0_f32] {
                    if dx == 0.0 && dy == 0.0 { continue; }
                    draw_spaced_text(painter, egui::pos2(content_x + dx, y + dy), "ANIMA", title_font.clone(), blue(0.1), 0.15);
                }
            }
            draw_spaced_text(painter, egui::pos2(content_x, y), "ANIMA", title_font.clone(), blue(0.2), 0.15);
            draw_spaced_text(painter, egui::pos2(content_x, y), "ANIMA", title_font, cream(1.0), 0.15);
            y += 80.0;

            // Accent line — smooth gradient: blue 0.8 on left fading to transparent
            let line_width = 180.0_f32;
            let line_steps = 90; // 2px per step = smooth
            let step_w = line_width / line_steps as f32;
            for s in 0..line_steps {
                let t = 1.0 - (s as f32 / (line_steps - 1) as f32); // 1.0 → 0.0
                let strip = egui::Rect::from_min_size(
                    egui::pos2(content_x + s as f32 * step_w, y),
                    egui::vec2(step_w + 0.5, 2.0),
                );
                painter.rect_filled(strip, 0.0, blue(t * 0.8));
            }
            y += 18.0;

            // Taglines
            draw_spaced_text(painter, egui::pos2(content_x, y), "POST-APOCALYPTIC · COLORADO · 2031", chakra(11.0), cream(0.4), 0.1);
            y += 18.0;
            draw_spaced_text(painter, egui::pos2(content_x, y), "SURVIVE. BUILD. REMEMBER.", chakra(11.0), cream(0.4), 0.1);
            y += 40.0;

            // Store position for the menu Area
            menu_anchor_x = content_x;
            menu_anchor_y = y;

            // --- Scanline overlay — faint horizontal lines across entire screen ---
            let scanline_color = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 18);
            let mut scan_y = rect.top();
            while scan_y < rect.bottom() {
                ui.painter().line_segment(
                    [egui::pos2(rect.left(), scan_y), egui::pos2(rect.right(), scan_y)],
                    egui::Stroke::new(1.0, scanline_color),
                );
                scan_y += 3.0;
            }
        });

    // Menu items — separate egui Window for guaranteed interaction
    let menu_items = ["PLAY", "SETTINGS", "EXIT"];
    let num_items = menu_items.len();

    // Keyboard navigation
    if keys.just_pressed(KeyCode::ArrowDown) || keys.just_pressed(KeyCode::Tab) {
        menu_sel.0 = (menu_sel.0 + 1) % num_items;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        menu_sel.0 = if menu_sel.0 == 0 { num_items - 1 } else { menu_sel.0 - 1 };
    }

    // Enter/Space activates selected item
    let kb_activate = keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space);

    egui::Area::new(egui::Id::new("main_menu_items"))
        .fixed_pos(egui::pos2(menu_anchor_x, menu_anchor_y))
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.hovered.bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.active.bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.hovered.weak_bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.active.weak_bg_fill = egui::Color32::TRANSPARENT;

            for (i, raw) in menu_items.iter().enumerate() {
                let selected = menu_sel.0 == i;

                let color = if selected { cream(0.95) } else { cream(0.5) };
                let text = egui::RichText::new(*raw)
                    .font(cinzel_bold(15.0))
                    .color(color);

                let btn = ui.add(
                    egui::Button::new(text)
                        .frame(false)
                        .min_size(egui::vec2(300.0, 28.0)),
                );

                // Mouse hover updates selection
                if btn.hovered() {
                    menu_sel.0 = i;
                }

                // Blue gradient indicator line for selected item
                if selected {
                    if let Some(tex_id) = line_tex {
                        let line_w = 36.0;
                        let line_h = 2.0;
                        let line_rect = egui::Rect::from_min_size(
                            egui::pos2(btn.rect.left() - line_w - 8.0, btn.rect.center().y - line_h / 2.0),
                            egui::vec2(line_w, line_h),
                        );
                        ui.painter().image(
                            tex_id,
                            line_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                }

                let activated = btn.clicked() || (selected && kb_activate);
                if activated {
                    match i {
                        0 => {
                            info!("Menu: {} — entering game", raw);
                            next_state.set(AppState::InGame);
                        }
                        // 1 => Settings (not yet implemented)
                        2 => std::process::exit(0),
                        _ => {}
                    }
                }
            }
        });
}

// ========================================
// InGame enter
// ========================================

fn despawn_menu(
    mut commands: Commands,
    camera_query: Query<Entity, With<MenuCamera>>,
    music: Option<Res<MenuMusicHandle>>,
    mut audio_instances: ResMut<Assets<AudioInstance>>,
) {
    // Keep the Camera2d alive for egui HUD rendering — just remove the menu marker.
    for e in camera_query.iter() {
        commands.entity(e).remove::<MenuCamera>();
    }
    // Fade out menu music
    if let Some(music) = music {
        if let Some(instance) = audio_instances.get_mut(&music.0) {
            instance.stop(AudioTween::linear(Duration::from_secs(2)));
        }
        commands.remove_resource::<MenuMusicHandle>();
    }
}

fn connect_to_server(mut commands: Commands, identity: Res<multiplayer::auth::ClientIdentity>) {
    // Default to production server; override with ANIMA_SERVER_ADDR for local dev
    let server_ip: Ipv4Addr = std::env::var("ANIMA_SERVER_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or([146, 71, 85, 180].into());
    let server_addr = SocketAddr::new(server_ip.into(), SERVER_PORT);
    let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0);

    let auth = Authentication::Manual {
        server_addr,
        client_id: identity.client_id,
        private_key: [0; 32],
        protocol_id: PROTOCOL_ID,
    };

    info!("Connecting as {} (id={})", identity.address, identity.client_id);

    let netcode_config = NetcodeConfig {
        client_timeout_secs: 120,
        token_expire_secs: 120,
        ..default()
    };

    let client_entity = commands
        .spawn((
            Client::default(),
            Link::default(),
            NetcodeClient::new(auth, netcode_config).expect("Failed to create netcode client"),
            UdpIo::default(),
            LocalAddr(client_addr),
            PeerAddr(server_addr),
            ReplicationReceiver::default(),
            PredictionManager::default(),
            ReplicationSender::new(
                Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
                SendUpdatesMode::SinceLastAck,
                false,
            ),
        ))
        .id();

    commands.trigger(Connect { entity: client_entity });

    // Store the client entity so we can send wallet auth after connection
    commands.insert_resource(PendingWalletAuth(client_entity));
}

/// Resource tracking that we need to send wallet auth on the client entity.
/// Consumed once the auth message is sent.
#[derive(Resource)]
struct PendingWalletAuth(Entity);

/// Client-side system: sends wallet auth message to server after connection.
/// Signs "ANIMA_AUTH_v1:{client_id}" with the Ed25519 keypair and sends
/// the pubkey + signature via the AuthChannel for server verification.
fn send_wallet_auth(
    pending: Option<Res<PendingWalletAuth>>,
    mut sender_query: Query<(&mut MessageSender<multiplayer::protocol::WalletAuthMessage>, Has<Connected>)>,
    identity: Res<multiplayer::auth::ClientIdentity>,
    mut commands: Commands,
) {
    let Some(pending) = pending else { return; };
    let Ok((mut sender, is_connected)) = sender_query.get_mut(pending.0) else {
        return;
    };
    if !is_connected { return; }

    // Sign and send wallet auth
    let (pubkey, signature) = identity.sign_auth();
    let auth_msg = multiplayer::protocol::WalletAuthMessage { pubkey, signature };

    sender.send::<multiplayer::protocol::AuthChannel>(auth_msg);
    info!(
        "[AUTH] Sent wallet auth to server (pubkey: {}, id: {})",
        identity.address, identity.client_id
    );

    // Remove resource — auth sent, don't send again
    commands.remove_resource::<PendingWalletAuth>();
}

// ========================================
// Player spawn
// ========================================

/// Log health changes for debugging.
fn log_health_changes(
    query: Query<(Entity, &PlayerHealth, Has<Controlled>), Changed<PlayerHealth>>,
) {
    for (entity, health, is_local) in query.iter() {
        let tag = if is_local { "LOCAL" } else { "REMOTE" };
        info!("[HEALTH] {} {:?} health={}", tag, entity, health.0);
    }
}

/// HUD: health bar at the bottom-center of the screen.
fn health_hud(
    mut contexts: EguiContexts,
    player_query: Query<&PlayerHealth, With<Controlled>>,
) {
    let Ok(health) = player_query.single() else { return; };
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    let screen = ctx.screen_rect();
    let bar_w = 200.0;
    let bar_h = 16.0;
    let bar_x = (screen.width() - bar_w) / 2.0;
    let bar_y = screen.height() - 50.0;

    egui::Area::new(egui::Id::new("health_hud"))
        .fixed_pos(egui::pos2(bar_x, bar_y))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            let hp = health.0.max(0) as f32;
            let pct = (hp / 100.0).clamp(0.0, 1.0);

            // Bar color: green → yellow → red as health drops
            let color = if pct > 0.5 {
                egui::Color32::from_rgb(50, 200, 80)
            } else if pct > 0.25 {
                egui::Color32::from_rgb(220, 180, 30)
            } else {
                egui::Color32::from_rgb(220, 40, 40)
            };

            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(bar_w, bar_h),
                egui::Sense::hover(),
            );

            // Background
            ui.painter().rect_filled(rect, 4.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160));
            // Health fill
            let fill_rect = egui::Rect::from_min_size(
                rect.min,
                egui::vec2(bar_w * pct, bar_h),
            );
            ui.painter().rect_filled(fill_rect, 4.0, color);
            // Border
            ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(1.0, egui::Color32::from_white_alpha(80)), egui::StrokeKind::Outside);
            // Text
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("{}", health.0.max(0)),
                chakra(12.0),
                egui::Color32::WHITE,
            );
        });
}

/// Crosshair — small cross at screen center when a gun is equipped.
fn crosshair_hud(
    mut contexts: EguiContexts,
    player_query: Query<&PlayerEquipped, With<Controlled>>,
) {
    let Ok(equipped) = player_query.single() else { return; };
    let Some(ref name) = equipped.0 else { return; };
    // Only show crosshair for guns
    if !(name.contains("AK") || name.contains("ak") || name.contains("gun")) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    let screen = ctx.screen_rect();
    let center = egui::pos2(screen.width() / 2.0, screen.height() / 2.0);
    let color = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 180);
    let stroke = egui::Stroke::new(1.5, color);
    let size = 8.0;
    let gap = 3.0;

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("crosshair"),
    ));

    // Horizontal lines
    painter.line_segment([egui::pos2(center.x - size, center.y), egui::pos2(center.x - gap, center.y)], stroke);
    painter.line_segment([egui::pos2(center.x + gap, center.y), egui::pos2(center.x + size, center.y)], stroke);
    // Vertical lines
    painter.line_segment([egui::pos2(center.x, center.y - size), egui::pos2(center.x, center.y - gap)], stroke);
    painter.line_segment([egui::pos2(center.x, center.y + gap), egui::pos2(center.x, center.y + size)], stroke);
    // Center dot
    painter.circle_filled(center, 1.0, color);
}

/// Inventory HUD — bottom-left, shows equipped item and carried inventory.
fn inventory_hud(
    mut contexts: EguiContexts,
    player_query: Query<(&PlayerEquipped, &PlayerInventory), With<Controlled>>,
    mut frame_count: Local<u32>,
) {
    *frame_count += 1;
    if *frame_count <= 2 { return; }
    let Ok((equipped, inventory)) = player_query.single() else { return; };
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    // Only show if the player has something equipped or in inventory
    if equipped.0.is_none() && inventory.items.is_empty() {
        return;
    }

    let screen = ctx.screen_rect();

    egui::Area::new(egui::Id::new("inventory_hud"))
        .fixed_pos(egui::pos2(16.0, screen.height() - 140.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            let bg = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 140);
            let frame = egui::Frame::NONE
                .fill(bg)
                .inner_margin(egui::Margin::same(8))
                .corner_radius(4.0);

            frame.show(ui, |ui| {
                // Equipped item
                if let Some(ref name) = equipped.0 {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(">")
                                .font(chakra(13.0))
                                .color(egui::Color32::from_rgb(100, 160, 255)),
                        );
                        ui.label(
                            egui::RichText::new(name)
                                .font(chakra(13.0))
                                .color(egui::Color32::WHITE),
                        );
                    });
                }

                // Inventory items
                if !inventory.items.is_empty() {
                    if equipped.0.is_some() {
                        ui.add_space(2.0);
                        ui.separator();
                        ui.add_space(2.0);
                    }
                    for item in &inventory.items {
                        ui.label(
                            egui::RichText::new(format!("  {}", item))
                                .font(chakra(11.0))
                                .color(cream(0.6)),
                        );
                    }
                }
            });
        });
}

/// Build version — bottom-right corner, always visible, muted gray.
/// Version from Cargo.toml + short git commit hash baked in at compile time.
fn build_version_hud(mut contexts: EguiContexts) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    let screen = ctx.screen_rect();

    let version = concat!("v", env!("CARGO_PKG_VERSION"), "-", env!("GIT_SHORT_HASH"));

    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("build_version"),
    ))
    .text(
        egui::pos2(screen.right() - 12.0, screen.bottom() - 14.0),
        egui::Align2::RIGHT_BOTTOM,
        version,
        chakra(10.0),
        egui::Color32::from_rgba_unmultiplied(180, 180, 180, 60),
    );
}

/// Death screen overlay — shown when the controlled player has PlayerDead.
/// Respawn delay must match server's RESPAWN_DELAY.
const RESPAWN_DELAY: f32 = 20.0;

fn death_screen(
    mut contexts: EguiContexts,
    player_query: Query<Has<multiplayer::protocol::PlayerDead>, With<Controlled>>,
    time: Res<Time>,
    mut death_start: Local<Option<f32>>,
    mut frame_count: Local<u32>,
) {
    *frame_count += 1;
    if *frame_count <= 2 { return; }
    let Ok(is_dead) = player_query.single() else { return; };

    if !is_dead {
        *death_start = None;
        return;
    }

    let now = time.elapsed_secs();
    let start = *death_start.get_or_insert(now);
    let elapsed = now - start;
    let remaining = (RESPAWN_DELAY - elapsed).max(0.0).ceil() as u32;

    let Ok(ctx) = contexts.ctx_mut() else { return; };

    let screen = ctx.screen_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("death_overlay")));

    // Dark red overlay
    painter.rect_filled(
        screen,
        0.0,
        egui::Color32::from_rgba_unmultiplied(80, 0, 0, 140),
    );
    // "YOU DIED" text
    painter.text(
        screen.center(),
        egui::Align2::CENTER_CENTER,
        "YOU DIED",
        cinzel_black(72.0),
        egui::Color32::from_rgb(220, 40, 40),
    );
    // Countdown timer
    painter.text(
        egui::pos2(screen.center().x, screen.center().y + 60.0),
        egui::Align2::CENTER_CENTER,
        format!("Respawning in {}s", remaining),
        chakra(16.0),
        cream(0.5),
    );
}

/// Kill feed display — shows recent kills at bottom-center of screen.
/// KillFeedEntry entities are spawned by the server and replicated.
const KILL_FEED_DURATION: f32 = 5.0;

fn kill_feed_ui(
    mut contexts: EguiContexts,
    feed_query: Query<&multiplayer::protocol::KillFeedEntry>,
    time: Res<Time>,
    mut frame_count: Local<u32>,
) {
    *frame_count += 1;
    if *frame_count <= 2 { return; }
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    let now = time.elapsed_secs();
    let screen = ctx.screen_rect();

    // Collect recent kills (within KILL_FEED_DURATION seconds)
    let mut entries: Vec<&multiplayer::protocol::KillFeedEntry> = feed_query
        .iter()
        .filter(|e| now - e.timestamp < KILL_FEED_DURATION)
        .collect();
    entries.sort_by(|a, b| b.timestamp.partial_cmp(&a.timestamp).unwrap());

    if entries.is_empty() { return; }

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("kill_feed"),
    ));

    for (i, entry) in entries.iter().take(5).enumerate() {
        let y = screen.bottom() - 90.0 - (i as f32 * 24.0);
        let alpha = ((KILL_FEED_DURATION - (now - entry.timestamp)) / KILL_FEED_DURATION).clamp(0.0, 1.0);

        // Background pill
        let text = format!("{} killed {}", entry.killer_name, entry.victim_name);
        let text_galley = painter.layout_no_wrap(text.clone(), chakra(13.0), cream(alpha));
        let text_w = text_galley.size().x;
        let pill_rect = egui::Rect::from_center_size(
            egui::pos2(screen.center().x, y),
            egui::vec2(text_w + 20.0, 22.0),
        );
        painter.rect_filled(
            pill_rect,
            11.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, (140.0 * alpha) as u8),
        );
        painter.text(
            egui::pos2(screen.center().x, y),
            egui::Align2::CENTER_CENTER,
            text,
            chakra(13.0),
            cream(alpha),
        );
    }
}

/// Predicted entity spawned — fires for our own player (which has Controlled).
/// Sets up physics for ALL predicted entities; cameras/input only for controlled ones.
fn on_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    query: Query<(&PlayerId, Has<Controlled>)>,
    position_query: Query<&avian3d::prelude::Position>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let entity = trigger.entity;
    let Ok((player_id, is_controlled)) = query.get(entity) else {
        return;
    };

    info!("[SPAWN] Predicted player {:?} (id={}) controlled={}", entity, player_id.0, is_controlled);

    let spawn_transform = position_query
        .get(entity)
        .map(|p| Transform::from_translation(p.0))
        .unwrap_or(Transform::from_translation(PLAYER_SPAWN_POS));

    // All predicted entities get physics + basic components
    commands.entity(entity).insert((
        player_physics_bundle(),
        Player { id: player_id.0 },
        spawn_transform,
        Visibility::default(),
    ));

    // Only our controlled entity gets cameras, input bindings, and view model
    if !is_controlled {
        return;
    }

    commands.entity(entity).insert(CameraSensitivity::default());

    let arm = meshes.add(Cuboid::new(0.1, 0.1, 0.5));
    let arm_material = materials.add(Color::from(tailwind::TEAL_200));

    commands.entity(entity).with_children(|parent| {
        parent.spawn((
            WorldModelCamera,
            Camera3d::default(),
            Projection::from(PerspectiveProjection {
                fov: 90.0_f32.to_radians(),
                ..default()
            }),
        ));
        parent.spawn((
            Camera3d::default(),
            Camera {
                order: 1,
                clear_color: ClearColorConfig::None,
                ..default()
            },
            Projection::from(PerspectiveProjection {
                fov: 70.0_f32.to_radians(),
                ..default()
            }),
            RenderLayers::layer(VIEW_MODEL_RENDER_LAYER),
        ));
        // Right hand (arm)
        parent.spawn((
            Mesh3d(arm),
            MeshMaterial3d(arm_material.clone()),
            Transform::from_xyz(0.2, -0.1, -0.25),
            RenderLayers::layer(VIEW_MODEL_RENDER_LAYER),
            NotShadowCaster,
        ));
        // Left hand — starts off-screen, animates in on jab
        parent.spawn((
            Mesh3d(meshes.add(Cuboid::new(0.12, 0.12, 0.4))),
            MeshMaterial3d(arm_material),
            Transform::from_xyz(-0.8, -0.3, -0.2),
            RenderLayers::layer(VIEW_MODEL_RENDER_LAYER),
            NotShadowCaster,
            LeftHand,
        ));
    });

    commands.spawn((
        ActionOf::<PlayerContext>::new(entity),
        Action::<MoveAction>::new(),
        Bindings::spawn(Cardinal::wasd_keys()),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(entity),
        Action::<JumpAction>::new(),
        Bindings::spawn(Spawn(Binding::from(KeyCode::Space))),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(entity),
        Action::<InteractAction>::new(),
        Bindings::spawn(Spawn(Binding::from(KeyCode::KeyE))),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(entity),
        Action::<DropAction>::new(),
        Bindings::spawn(Spawn(Binding::from(KeyCode::KeyG))),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(entity),
        Action::<JabAction>::new(),
        Bindings::spawn(Spawn(Binding::from(KeyCode::KeyQ))),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(entity),
        Action::<PrimaryAction>::new(),
        Bindings::spawn(Spawn(Binding::from(MouseButton::Left))),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(entity),
        Action::<LookAction>::new(),
        Bindings::spawn(Spawn(Binding::mouse_motion())),
    ));
}

/// Remote player: interpolated entity — smooth, slightly delayed, no rubberbanding.
/// Lightyear never adds Interpolated to our own entity, so no guards needed.
fn on_interpolated_spawn(
    trigger: On<Add, (PlayerId, Interpolated)>,
    query: Query<&PlayerId>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let entity = trigger.entity;
    let Ok(player_id) = query.get(entity) else {
        return;
    };

    info!("[SPAWN] Remote interpolated player spawned: {:?} (id={})", entity, player_id.0);

    commands.entity(entity).insert((
        player_physics_bundle(),
        Player { id: player_id.0 },
        Mesh3d(meshes.add(Capsule3d::default())),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
        Visibility::default(),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));
}
