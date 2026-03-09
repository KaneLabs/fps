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
    init_replicated_interactables, sync_door_state, sync_equippable_visibility,
    sync_remote_equipped, sync_remote_orientation,
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

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "ANIMA".to_string(),
            ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(Color::BLACK));
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
            mouse_look,
            sync_player_yaw,
            grab_mouse,
            change_fov,
            update_view_model,
            interaction_ui_system,
            sync_door_state,
            sync_remote_orientation,
            init_replicated_doors,
            init_replicated_equippables,
            init_replicated_interactables,
        )
            .run_if(in_state(AppState::InGame)),
    );
    app.add_systems(
        Update,
        (sync_equippable_visibility, sync_remote_equipped)
            .run_if(in_state(AppState::InGame))
            .run_if(not(lightyear::prelude::is_in_rollback)),
    );
    app.add_systems(
        FixedPreUpdate,
        pre_rotate_move_input
            .after(EnhancedInputSystems::Update)
            .before(lightyear::prelude::client::input::InputSystems::BufferClientInputs)
            .run_if(not(lightyear::prelude::is_in_rollback))
            .run_if(in_state(AppState::InGame)),
    );

    app.add_observer(on_predicted_spawn);
    app.add_observer(on_interpolated_spawn);
    app.run();
}

// ========================================
// Setup
// ========================================

fn setup(mut commands: Commands) {
    commands.spawn((MenuCamera, Camera2d));
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

/// Draw the menu title block: "PROJECT CODENAME" + "ANIMA" + accent line + subtitles.
fn draw_menu_title(ui: &mut egui::Ui) {
    // "PROJECT CODENAME" — small, spaced, blue at 0.7
    ui.label(
        egui::RichText::new("P R O J E C T   C O D E N A M E")
            .font(chakra(12.0))
            .color(blue(0.7)),
    );

    ui.add_space(8.0);

    // "ANIMA" — large Cinzel Bold, cream #F0E8D8
    // Title glow: two shadow layers behind the text
    let title_font = cinzel_black(72.0);
    let title_text = "ANIMA";

    // Paint glow layers first (wide blur approximation via offset text)
    let (_, title_rect) = ui.allocate_space(egui::vec2(ui.available_width(), 80.0));
    let title_pos = egui::pos2(title_rect.left(), title_rect.center().y);
    let painter = ui.painter();

    // Outer glow — blue at 0.1, painted at slight offsets
    for dx in [-2.0, 0.0, 2.0_f32] {
        for dy in [-1.0, 0.0, 1.0_f32] {
            if dx == 0.0 && dy == 0.0 { continue; }
            painter.text(
                title_pos + egui::vec2(dx, dy),
                egui::Align2::LEFT_CENTER,
                title_text, title_font.clone(), blue(0.1),
            );
        }
    }
    // Inner glow — blue at 0.2
    painter.text(title_pos, egui::Align2::LEFT_CENTER, title_text, title_font.clone(), blue(0.2));
    // Main title — cream
    painter.text(title_pos, egui::Align2::LEFT_CENTER, title_text, title_font, cream(1.0));

    ui.add_space(8.0);

    // Accent divider line — blue at 0.8, fading to transparent
    let (_, line_rect) = ui.allocate_space(egui::vec2(50.0, 2.0));
    ui.painter().rect_filled(line_rect, 0.0, blue(0.8));

    ui.add_space(16.0);

    // Taglines — cream at 0.4
    ui.label(
        egui::RichText::new("P O S T - A P O C A L Y P T I C  ·  C O L O R A D O  ·  2 0 3 1")
            .font(chakra(11.0))
            .color(cream(0.4)),
    );

    ui.add_space(4.0);

    ui.label(
        egui::RichText::new("S U R V I V E .   B U I L D .   R E M E M B E R .")
            .font(chakra(11.0))
            .color(cream(0.4)),
    );
}

// ========================================
// Loading state
// ========================================

fn loading_setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    let handles = vec![
        asset_server.load("ak47.glb"),
        asset_server.load("dirty-pickaxe.glb"),
        asset_server.load("ore_chunk.glb"),
    ];
    commands.insert_resource(AssetLoadTracker { handles });

    // Preload the Anima cover image for the menu
    let cover: Handle<Image> = asset_server.load("images/anima-cover-darker.png");
    commands.insert_resource(AnimaCover(cover));

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
    info!("Main menu entered — music playing");
}

fn menu_ui(
    mut contexts: EguiContexts,
    keys: Res<ButtonInput<KeyCode>>,
    mut next_state: ResMut<NextState<AppState>>,
    anima_cover: Option<Res<AnimaCover>>,
    mut frame_count: Local<u32>,
) {
    *frame_count += 1;
    if *frame_count <= 2 { return; }

    // Register the cover image with egui (must happen before ctx_mut borrow)
    let cover_tex = anima_cover.as_ref().map(|c| {
        contexts.add_image(bevy_egui::EguiTextureHandle::Strong(c.0.clone()))
    });

    let Ok(ctx) = contexts.ctx_mut() else { return; };

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
                "Alpha 0.1  ·  Build 2031",
                chakra(11.0),
                cream(0.2),
            );

            // --- Right side content area — anchored to right half of screen ---
            let half = rect.center().x;
            let padding = (rect.width() * 0.04).max(32.0); // 4% of screen, min 32px
            let content_x = half + padding;
            let content_width = rect.right() - content_x - padding;
            let content_rect = egui::Rect::from_min_size(
                egui::pos2(content_x, rect.top()),
                egui::vec2(content_width, rect.height()),
            );

            let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
            child_ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                // Vertically center the content block
                let space = (ui.available_height() - 400.0).max(0.0) * 0.38;
                ui.add_space(space);

                draw_menu_title(ui);

                ui.add_space(40.0);

                // Menu items: PLAY, SETTINGS, EXIT
                let menu_items: &[(&str, &str, &str)] = &[
                    ("PLAY",     "P L A Y",         "P  L  A  Y"),
                    ("SETTINGS", "S E T T I N G S", "S  E  T  T  I  N  G  S"),
                    ("EXIT",     "E X I T",         "E  X  I  T"),
                ];

                for (i, (raw, normal, wide)) in menu_items.iter().enumerate() {
                    // Allocate a clickable/hoverable rect
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(220.0, 28.0),
                        egui::Sense::click(),
                    );

                    let hovered = response.hovered();

                    // Draw text — wider spacing + brighter on hover
                    let (label, color) = if hovered {
                        (*wide, cream(0.95))
                    } else {
                        (*normal, cream(0.5))
                    };
                    ui.painter().text(
                        rect.left_center(),
                        egui::Align2::LEFT_CENTER,
                        label,
                        chakra_bold(15.0),
                        color,
                    );

                    // Blue indicator line shoots out on hover
                    if hovered {
                        let line_x = rect.left() - 12.0;
                        let line_y = rect.center().y;
                        ui.painter().line_segment(
                            [
                                egui::pos2(line_x - 24.0, line_y),
                                egui::pos2(line_x, line_y),
                            ],
                            egui::Stroke::new(2.0, blue(0.8)),
                        );
                    }

                    if response.clicked() {
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

            // --- Scanline overlay — faint horizontal lines across entire screen ---
            let scanline_color = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 18);
            let mut y = rect.top();
            while y < rect.bottom() {
                ui.painter().line_segment(
                    [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                    egui::Stroke::new(1.0, scanline_color),
                );
                y += 3.0;
            }
        });

    // Enter key also starts the game
    if keys.just_pressed(KeyCode::Enter) {
        next_state.set(AppState::InGame);
    }
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
    for e in camera_query.iter() {
        commands.entity(e).despawn();
    }
    // Fade out menu music
    if let Some(music) = music {
        if let Some(instance) = audio_instances.get_mut(&music.0) {
            instance.stop(AudioTween::linear(Duration::from_secs(2)));
        }
        commands.remove_resource::<MenuMusicHandle>();
    }
}

fn connect_to_server(mut commands: Commands) {
    let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), SERVER_PORT);
    let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0);

    let client_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let auth = Authentication::Manual {
        server_addr,
        client_id,
        private_key: [0; 32],
        protocol_id: PROTOCOL_ID,
    };

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
                Duration::from_millis(100),
                SendUpdatesMode::SinceLastAck,
                false,
            ),
        ))
        .id();

    commands.trigger(Connect { entity: client_entity });
    info!("Connecting to server at {} as client {}", server_addr, client_id);
}

// ========================================
// Player spawn
// ========================================

/// Local player: predicted entity owned by this client.
fn on_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    existing_local: Query<(), With<LocalPlayer>>,
    query: Query<&PlayerId>,
    position_query: Query<&avian3d::prelude::Position>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let entity = trigger.entity;
    let Ok(player_id) = query.get(entity) else {
        return;
    };

    if existing_local.get(entity).is_ok() {
        return;
    }
    info!("Local player spawned: {:?}", entity);

    let spawn_transform = position_query
        .get(entity)
        .map(|p| Transform::from_translation(p.0))
        .unwrap_or(Transform::from_translation(PLAYER_SPAWN_POS));

    let arm = meshes.add(Cuboid::new(0.1, 0.1, 0.5));
    let arm_material = materials.add(Color::from(tailwind::TEAL_200));

    commands.entity(entity).insert((
        player_physics_bundle(),
        LocalPlayer,
        CameraSensitivity::default(),
        Player { id: player_id.0 },
        spawn_transform,
        Visibility::default(),
    ));

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
        parent.spawn((
            Mesh3d(arm),
            MeshMaterial3d(arm_material),
            Transform::from_xyz(0.2, -0.1, -0.25),
            RenderLayers::layer(VIEW_MODEL_RENDER_LAYER),
            NotShadowCaster,
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
        Action::<MineAction>::new(),
        Bindings::spawn(Spawn(Binding::from(MouseButton::Left))),
    ));
}

/// Remote player: interpolated entity — smooth, slightly delayed, no rubberbanding.
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

    info!("Remote player spawned (interpolated): {:?} (id={})", entity, player_id.0);

    commands.entity(entity).insert((
        player_physics_bundle(),
        Player { id: player_id.0 },
        Mesh3d(meshes.add(Capsule3d::default())),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
        Visibility::default(),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));
}
