// Pass 2: inverted-hull outlines, outlines v2 (M1 C4, spec §5.12 "Outlines"): the anime line art.
// The pipeline culls front faces, so only the back of a hull pushed outward along the smoothed
// outline normal survives, peeking out around the silhouette and over the parts a character's
// other parts overlap. The outline program is frame.wgsl, materials.wgsl, lights.wgsl and
// anime_character.wgsl, then this file: lines are lit through the anime character shader's own
// functions, and its group 3.
//
// Width, in pixels on a 1080-row target, scaled with the target's height:
//
//   width × region scale × vertex color G × model scale × curve(d) × fade(d)
//
// The material's width (`AnimeCharacterParams::edges.z`) is the width at the curve's mid
// distance; the region (the vertex's material ID) scales it. `d` is the effective distance: the
// view depth × tan(fov_y / 2) / tan(22.5°), the distance at which the model would look as large
// through a 45° lens, so a line keeps its width wherever a character looks the same size, however
// it got there (zooming in is moving closer). The curve (`outline_curve`) has two linear segments
// between its near, mid and far knots and keeps the line's width in the world outside them:
// readable in close-ups, thinning with distance without vanishing early. Over the last quarter of
// the material's LOD cutoff the width fades to nothing; past it the renderer draws no hull.
//
// Extrusion, in view space: the outline normal's part across the screen, with the material's
// flattening behind it (away from the camera) in place of its own depth, normalized (view-space Z
// flattening), times the width in meters at the vertex's depth. Normals across the screen extrude
// the full width; normals facing the camera extrude less and sink behind the surface instead of
// poking through it at grazing angles. The hull is also pushed away from the camera along the
// view ray by the material's Z offset × the vertex color's B (meters of view depth), which hides
// it behind nearby surfaces without moving it on screen. All of it goes through one unjittered
// matrix into clip space (`outline_clip`), so the prepass places a hull vertex in the last frame
// as exactly as in this one.
//
// Color: the region's outline color, blended toward that color × the albedo by the material's
// albedo weight (a darker shade of the surface, as anime color lines are), then lit as the
// surface it outlines is: the character's key light through the material's light split (on the
// outline normal) and its diffuse mode's shadow color (`shade_tint`, bands or ramp), plus the
// rig's stylized GI flattened toward the camera (`character_gi` with the view direction as the
// line's normal; in regions with a sphere weight, faces' and hair's, that normal turns toward the
// head sphere's by the weight, as their surfaces' do), by the material's lighting weight. Lines
// darken at night and take the light's tint, and go into a cast shadow with their surface (M1
// R4: `cast_shadow_at`, a face's whole). Local lights don't reach them; outline hulls cast no
// shadow. Where the material has a control map its material ID picks the region's color per
// pixel, as on the surface; the width scale is the vertex's region's (the vertex stage reads no
// textures). Alpha is the character's, post's character mask (M1 P2): 0 on a character's line,
// as on the character it outlines, so the line belongs to the character's bloom, and 1 on an
// unlisted instance's. Last, the line fogs like its surface (M1 W3): its character's share of the
// frame's fog at the hull.
//
// Velocity: the scene prepass draws the hulls too (`vs_outline_prepass`), after the surfaces,
// with this frame's and the last frame's clip positions, and its depth is the main pass's. A
// hull vertex moves with its surface vertex (the last drawn pose, slot 4), its instance's last
// transform and the last frame's camera. A skinned outline normal is this frame's in both: the
// skinning outputs keep last frame's positions, not normals. A hull pixel writes its line's
// velocity, the motion of the silhouette it draws rather than of the hull's point there
// (`fs_outline_prepass`, M1 P1), so temporal AA, and any upscaler that replaces it, reprojects
// the line with the silhouette. The hulls also write the prepass's outline mask, which makes
// lines fully reactive under temporal AA: a moving line is drawn from the current frame.
//
// The alpha-tested permutation discards the hull where the base color map's coverage is below
// the material's cutoff, so cutouts get no outline across their holes, and its prepass does too
// (`vs_outline_prepass_masked`, M1 C7), as the surfaces' masked prepasses do (prepass.wgsl).
// Transparent materials draw no hull.
//
// A fading character's hulls (M1 C7) draw through the alpha-tested permutation and keep the
// pixels its surfaces keep (the proximity fade's dither, frame.wgsl), under MSAA through
// `fs_outline_covered`'s sample mask, and in the scene prepass through the masked pipeline.

// The width's reference (`OUTLINE_REFERENCE_*`) and its curve (`outline_curve`) are
// anime_character.wgsl's, which the depth-offset rim's width follows too (M1 C5).
// How many line widths a hull pixel's velocity follows the facing's gradient to the silhouette
// (`fs_outline_prepass`): the facing grows like a square root of the distance from the
// silhouette, so a linear estimate from inside the line overshoots it (by 1.6 times at a sphere
// line's inner edge).
const OUTLINE_SILHOUETTE_REACH: f32 = 2.0;

struct OutlineVaryings {
    // Invariant, so the hull's prepass (`vs_outline_prepass`) computes the same depth.
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) @interpolate(flat) material: u32,
    // The region the vertex's material ID picks, clamped to the material's.
    @location(1) @interpolate(flat) region: u32,
    // The material textures' coordinates.
    @location(2) uv: vec2<f32>,
    // The instance's entry in the frame's characters: its light rig.
    @location(3) @interpolate(flat) character: u32,
    @location(4) world_position: vec3<f32>,
    // The outline normal in world space, which the line's light split reads.
    @location(5) world_normal: vec3<f32>,
    // The vertex color's ambient occlusion.
    @location(6) occlusion: f32,
}

// A hull vertex before it is extruded: where its surface vertex is, in world space, and its
// outline normal, of any length.
struct HullVertex {
    position: vec3<f32>,
    normal: vec3<f32>,
}

// What sets a hull vertex's extrusion, the same in every frame.
struct HullWidth {
    // Pixels on a 1080-row target at the curve's mid distance: the material's width × the
    // region's scale × the vertex color's G × the model scale.
    pixels: f32,
    // Meters of view depth away from the camera, along the view ray: the material's Z offset ×
    // the vertex color's B.
    z_offset: f32,
    material: u32,
}

// The vertex's region: its material ID, clamped to the material's regions.
fn outline_region(vertex: MeshVertex, material: u32) -> u32 {
    let regions = max(materials[material].modes.y, 1u);
    return min(vertex.flags & VERTEX_MATERIAL_ID_MASK, regions - 1u);
}

fn hull_width(vertex: MeshVertex, material: u32, region: u32) -> HullWidth {
    // Read field by field rather than copying the whole block.
    var width: HullWidth;
    width.pixels = materials[material].edges.z * materials[material].regions[region].outline.w
        * vertex.color.g * materials[material].outline_shape.z;
    width.z_offset = materials[material].edges.w * vertex.color.b;
    width.material = material;
    return width;
}

// The hull vertex of an instance placed by `world_from_object`. Normals take the inverse
// transpose of its linear part; the cofactor matrix points them the same way without an inverse
// (the extrusion normalizes), and computing it from the transform alone, in this frame and the
// last, gives a still instance's hull exactly no motion.
fn hull_vertex(world_from_object: mat4x4<f32>, position: vec3<f32>, outline_normal: vec3<f32>) -> HullVertex {
    let x = world_from_object[0].xyz;
    let y = world_from_object[1].xyz;
    let z = world_from_object[2].xyz;
    let cofactor = mat3x3<f32>(cross(y, z), cross(z, x), cross(x, y));
    var hull: HullVertex;
    hull.position = (world_from_object * vec4<f32>(position, 1.0)).xyz;
    hull.normal = cofactor * outline_normal;
    return hull;
}

// Where `hull` lands, extruded, through `clip_from_world`: an unjittered matrix into clip space
// whose projection is CameraData's (perspective, infinite reverse-Z, so clip x and y are the
// focal lengths × view x and y, clip z the near plane's distance and clip w the view depth).
// Everything is read from the matrix, so last frame's places last frame's hull.
//
// The extrusion's part across the screen moves the vertex at its own depth; its depth part (the
// flattening's) and the Z offset push it back along the view ray, which scales clip x, y and w
// together and so never moves it on screen. Pushing along view −z instead would pull vertices
// toward the screen's center by their distance from it, thinning lines at the edges of a wide
// field of view.
fn outline_clip(clip_from_world: mat4x4<f32>, hull: HullVertex, width: HullWidth) -> vec4<f32> {
    let clip = clip_from_world * vec4<f32>(hull.position, 1.0);
    let direction = clip_from_world * vec4<f32>(hull.normal, 0.0);
    let focal = focal_lengths(clip_from_world);
    // The outline normal in view space (x right, y up, z toward the camera), then flattened: its
    // part across the screen, with the material's flattening away from the camera as its depth.
    let normal = safe_normalize(vec3<f32>(direction.xy / focal, -direction.w));
    let extrusion = safe_normalize(vec3<f32>(normal.xy, -materials[width.material].outline_shape.w));
    let depth = max(clip.w, 1e-4);
    let distance = effective_distance(depth, focal.y);
    let pixels = width.pixels * (view.viewport.y / OUTLINE_REFERENCE_HEIGHT) * outline_curve(width.material, distance);
    // Pixels to view-space meters at this depth: the target's height spans 2 × depth / focal y.
    let meters = pixels * 2.0 * depth / (focal.y * view.viewport.y);
    let across = clip.xy + extrusion.xy * focal * meters;
    // Back along the view ray by the Z offset and the extrusion's depth, in meters of depth.
    let back = 1.0 + max(width.z_offset - extrusion.z * meters, -0.5 * depth) / depth;
    return vec4<f32>(across * back, clip.z, clip.w * back);
}

// The focal lengths of `clip_from_world` (see `outline_clip`): the rows of clip x and y have
// their lengths, since the view is rigid.
fn focal_lengths(clip_from_world: mat4x4<f32>) -> vec2<f32> {
    return vec2<f32>(
        length(vec3<f32>(clip_from_world[0].x, clip_from_world[1].x, clip_from_world[2].x)),
        length(vec3<f32>(clip_from_world[0].y, clip_from_world[1].y, clip_from_world[2].y)),
    );
}

// The effective distance (see the top of this file) at view depth `depth`, through a vertical
// focal length `focal_y`.
fn effective_distance(depth: f32, focal_y: f32) -> f32 {
    return depth / (focal_y * OUTLINE_REFERENCE_TAN_HALF_FOV);
}

// `clip` with this frame's jitter (M1 R7), as the jittered projection would have it.
fn jittered(clip: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(clip.xy + view.jitter.zw * clip.w, clip.zw);
}

@vertex
fn vs_outline(packed: PackedVertex, draw: DrawRecord) -> OutlineVaryings {
    return outline_varyings(packed, draw);
}

// A hull vertex, for the main pass's entry point and the material debug views' (M1 F5).
fn outline_varyings(packed: PackedVertex, draw: DrawRecord) -> OutlineVaryings {
    let vertex = decode_vertex(packed, draw);
    let instance = instances[draw.instance];
    let region = outline_region(vertex, draw.material);
    let hull = hull_vertex(instance.world_from_object, vertex.position, vertex.outline_normal);
    var out: OutlineVaryings;
    out.clip_position =
        jittered(outline_clip(view.unjittered_clip_from_world, hull, hull_width(vertex, draw.material, region)));
    out.material = draw.material;
    out.region = region;
    out.uv = vertex.uv;
    out.character = instance.character;
    out.world_position = hull.position;
    out.world_normal = hull.normal;
    out.occlusion = vertex.color.r;
    return out;
}

// A hull pixel (`fs_outline`): its final color and alpha, whether it is kept, and the region
// whose line it draws (the material debug views', M1 F5).
struct OutlinePixel {
    color: vec4<f32>,
    kept: bool,
    region: u32,
}

fn outline_pixel(in: OutlineVaryings) -> OutlinePixel {
    let material = in.material;
    let modes = materials[material].modes;
    let swizzles = materials[material].swizzles;
    let style = materials[material].outline_color;
    // Derivatives first, in uniform control flow: the base color map is sampled on a branch.
    let uv = MaterialUv(in.uv, dpdx(in.uv), dpdy(in.uv));
    let rig = characters[in.character].rig;
    // A cast shadow (M1 R4) joins the occlusion as on the surface, so a line goes into the shadow
    // tone with it: a face's whole (the face pass's), any other surface's at the hull. Both are
    // computed and one selected, without a branch around the lookups (as for WARP below).
    let face = materials[material].modes.z == CHARACTER_SURFACE_FACE;
    let at_hull = cast_shadow_at(in.world_position, in.world_normal, in.character);
    let shadow = select(at_hull, face_shadow(in.character), face);
    let x = split_input(dot(safe_normalize(in.world_normal), rig.key_direction.xyz), in.occlusion * shadow, SHADOW_BIAS_NEUTRAL);
    let split = materials[material].split.xy;
    let width = max(split.y, 0.5 * fwidth(x));

    // The albedo, for the alpha test and the line's color: one sample serves both.
    var base = materials[material].base_color;
    if material_has_texture(modes, CHARACTER_BASE_COLOR) && (PERMUTATION_ALPHA_TEST || style.x > 0.0) {
        base *= sample_material(base_color_map, swizzles, CHARACTER_BASE_COLOR, uv);
    }
    // The cutout, of alpha-tested materials: the permutation also draws fading characters' opaque
    // ones (M1 C7), whose dither it then follows, decided after every derivative. Whole quads
    // leave here (`quad_any`, frame.wgsl), single pixels at the entry points' one exit
    // (docs/conventions.md).
    var kept = true;
    if PERMUTATION_ALPHA_TEST {
        let alpha_tested = (modes.x & CHARACTER_FLAG_ALPHA_TEST) != 0u;
        let fade = character_fade(in.character);
        let cut = alpha_tested && base.a < materials[material].split.w;
        kept = !cut && !(fade > 0.0 && faded_out(fade, in.clip_position.xy, in.character, FADE_SAMPLES));
        if !quad_any(kept) {
            discard;
        }
    }
    var region = in.region;
    if material_has_texture(modes, CHARACTER_CONTROL) {
        region = control_material_id(swizzles, in.uv);
    }
    region = min(region, max(modes.y, 1u) - 1u);
    var surface: CharacterSurface;
    surface.region = materials[material].regions[region];
    let line = surface.region.outline.rgb * mix(vec3<f32>(1.0), base.rgb, style.x);
    // The world's share of the pixel, as the surface writes it (anime_character.wgsl).
    let world = select(1.0, 0.0, in.character != 0u);

    // Lit as the surface is: the key light through the split and the shadow color on the
    // outline normal, plus the GI flattened toward the camera. Always computed, and then
    // weighed, rather than returned early for unlit lines: an early return here crashed WARP's
    // shader compiler (D3D12; M1 C4's dx12-warp goldens).
    let view_dir = safe_normalize(view.position.xyz - in.world_position);
    surface.albedo = base.rgb;
    surface.normal = view_dir;
    surface.view_dir = view_dir;
    surface.occlusion = in.occlusion;
    surface.shadow_bias = SHADOW_BIAS_NEUTRAL;
    surface.split = split;
    let lit = light_step(split.x, width, x);
    let tone = mix(shade_tint(surface, material, x, width), vec3<f32>(1.0), lit);
    let gi = character_gi(surface, rig, characters[in.character].ambient, in.world_position);
    let lit_line = line * (tone * rig.key_radiance.rgb + gi);
    // Unlit lines are their color exactly, and every line takes the fog its surface does (M1 W3),
    // at the hull.
    let line_color = select(mix(line, lit_line, style.y), line, style.y <= 0.0);
    // The final color, in one variable at the one exit.
    let color = vec4<f32>(fogged(line_color, in.world_position, fog_share(in.character)), world);
    return OutlinePixel(color, kept, region);
}

@fragment
fn fs_outline(in: OutlineVaryings) -> @location(0) vec4<f32> {
    let pixel = outline_pixel(in);
    // The one exit, where the pixels cut out leave (docs/conventions.md); never in the opaque
    // permutation (as the surfaces').
    if PERMUTATION_ALPHA_TEST && !pixel.kept {
        discard;
    }
    return pixel.color;
}

// The alpha-tested hulls under MSAA (M1 C7): a fading character's lines over the samples its
// surfaces cover.
@fragment
fn fs_outline_covered(in: OutlineVaryings) -> CoveredOutput {
    let pixel = outline_pixel(in);
    var out: CoveredOutput;
    out.color = pixel.color;
    out.mask = fade_mask(character_fade(in.character), in.clip_position.xy, in.character);
    // The one exit, where the pixels cut out leave.
    if !pixel.kept {
        discard;
    }
    return out;
}

// A hull vertex for the material debug views (M1 F5; see the end of anime_character.wgsl): the
// main pass's, with the line's width.
struct OutlineDebugVaryings {
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) @interpolate(flat) material: u32,
    @location(1) @interpolate(flat) region: u32,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) character: u32,
    @location(4) world_position: vec3<f32>,
    @location(5) world_normal: vec3<f32>,
    @location(6) occlusion: f32,
    // The line's width at the vertex in pixels on a 1080-row target, as its extrusion takes it:
    // the material's, the region's and the vertex's scales × the distance curve (`outline_clip`,
    // without the target's own height; the cutoff's fade, which fades the line rather than
    // narrowing it, is left out).
    @location(7) width: f32,
}

@vertex
fn vs_outline_debug(packed: PackedVertex, draw: DrawRecord) -> OutlineDebugVaryings {
    let main = outline_varyings(packed, draw);
    let vertex = decode_vertex(packed, draw);
    let clip_from_world = view.unjittered_clip_from_world;
    let width = hull_width(vertex, draw.material, main.region);
    let depth = max((clip_from_world * vec4<f32>(main.world_position, 1.0)).w, 1e-4);
    let distance = effective_distance(depth, focal_lengths(clip_from_world).y);
    var out: OutlineDebugVaryings;
    out.clip_position = main.clip_position;
    out.material = main.material;
    out.region = main.region;
    out.uv = main.uv;
    out.character = main.character;
    out.world_position = main.world_position;
    out.world_normal = main.world_normal;
    out.occlusion = main.occlusion;
    out.width = width.pixels * outline_curve(draw.material, distance);
    return out;
}

// A hull pixel's values: `fs_outline`'s, its width in w.
@fragment
fn fs_outline_debug(in: OutlineDebugVaryings) -> @location(0) vec4<f32> {
    let main = OutlineVaryings(
        in.clip_position,
        in.material,
        in.region,
        in.uv,
        in.character,
        in.world_position,
        in.world_normal,
        in.occlusion,
    );
    let pixel = outline_pixel(main);
    let values = character_debug_values(DEBUG_DRAWN_LINE, in.material, pixel.region, in.width);
    // The one exit, as the main pass's.
    if PERMUTATION_ALPHA_TEST && !pixel.kept {
        discard;
    }
    return values;
}

// The hull in the scene prepass: `ScenePrepassVaryings` (frame.wgsl), and how squarely the
// outline normal faces the camera, which the fragment stage follows to the silhouette.
struct OutlinePrepassVaryings {
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) current: vec4<f32>,
    @location(2) previous: vec4<f32>,
    // The outline normal's cosine with the direction to the camera at the surface vertex: 0 on
    // the silhouette, negative on the back of the hull that shows as the line.
    @location(3) facing: f32,
    // How far, in pixels, the extrusion moved the vertex across the screen: the line's width.
    @location(4) reach: f32,
}

// The hull in the scene prepass: depth, its outline normal and its velocity.
@vertex
fn vs_outline_prepass(
    packed: PackedVertex,
    // Slot 4: where the vertex was when last drawn, in slot 1's format. Static meshes bind
    // their own positions again.
    @location(10) previous_position: vec4<f32>,
    draw: DrawRecord,
) -> OutlinePrepassVaryings {
    return outline_prepass_varyings(packed, previous_position, draw);
}

// `vs_outline_prepass`'s vertex.
fn outline_prepass_varyings(packed: PackedVertex, previous_position: vec4<f32>, draw: DrawRecord) -> OutlinePrepassVaryings {
    let vertex = decode_vertex(packed, draw);
    let instance = instances[draw.instance];
    let width = hull_width(vertex, draw.material, outline_region(vertex, draw.material));
    let hull = hull_vertex(instance.world_from_object, vertex.position, vertex.outline_normal);
    let previous = hull_vertex(
        instance.previous_world_from_object,
        object_position(previous_position, draw),
        vertex.outline_normal,
    );
    var out: OutlinePrepassVaryings;
    out.current = outline_clip(view.unjittered_clip_from_world, hull, width);
    out.clip_position = jittered(out.current);
    out.previous = outline_clip(view.previous_clip_from_world, previous, width);
    out.world_normal = hull.normal;
    out.facing = dot(safe_normalize(hull.normal), safe_normalize(view.position.xyz - hull.position));
    let base = view.unjittered_clip_from_world * vec4<f32>(hull.position, 1.0);
    let across = out.current.xy / max(out.current.w, 1e-4) - base.xy / max(base.w, 1e-4);
    out.reach = length(across * 0.5 * view.viewport.xy);
    return out;
}

// The scene prepass's targets as the hull writes them: its normal and velocity, and the outline
// mask, which the responsive mask takes in (temporal.wgsl).
struct OutlinePrepassTargets {
    @location(0) normal: vec2<f32>,
    @location(1) velocity: vec2<f32>,
    @location(2) outline: f32,
}

// The velocity a hull pixel writes is the line's (M1 P1): the motion of the silhouette it draws,
// not of the hull's point there. Whenever a body turns against the view (about its own axis, or
// under a camera circling it), its points slide through the silhouette, which moves with the
// point on it alone: the line shows the back of the hull, points up to a line's width behind the
// silhouette, sliding the other way. Across the line the motion changes with how squarely the
// outline normal faces the camera, so the pixel follows its derivatives to where it faces
// across, the silhouette, at most a line's width: a turn's sliding goes, a translation or a limb
// swinging (the same motion everywhere, or the silhouette's own) stays.
@fragment
fn fs_outline_prepass(in: OutlinePrepassVaryings) -> OutlinePrepassTargets {
    return outline_prepass_targets(in);
}

// `fs_outline_prepass`'s targets.
fn outline_prepass_targets(in: OutlinePrepassVaryings) -> OutlinePrepassTargets {
    let current = in.current.xy / in.current.w;
    let previous = in.previous.xy / in.previous.w;
    // NDC is y up, texture coordinates y down, and both span 2 NDC units per texture.
    let velocity = (current - previous) * vec2<f32>(0.5, -0.5);
    // Derivatives first, in uniform control flow.
    let gradient = vec2<f32>(dpdx(in.facing), dpdy(in.facing));
    let along = dpdx(velocity) * gradient.x + dpdy(velocity) * gradient.y;
    let steepness = max(dot(gradient, gradient), 1e-12);
    // How far the silhouette is, in pixels, as the facing's gradient extrapolates it: farther
    // than the hull reaches across the screen, the estimate is unsure and is followed less.
    let distance = abs(in.facing) * inverseSqrt(steepness);
    let followed = min(1.0, OUTLINE_SILHOUETTE_REACH * in.reach / max(distance, 1e-6));
    let sliding = along * (in.facing * followed / steepness);
    // Below a thousandth of a pixel the derivatives' rounding is all there is: a hull that holds
    // still keeps exactly no velocity.
    let line = velocity - select(vec2<f32>(0.0), sliding, dot(sliding, sliding) > 1e-12);
    return OutlinePrepassTargets(oct_encode(safe_normalize(in.world_normal)), line, OUTLINE_MASK_LINE);
}

// An alpha-tested or fading character's hull in the scene prepass (M1 C7): `OutlinePrepassVaryings`,
// and what the cut needs.
struct MaskedOutlinePrepassVaryings {
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) current: vec4<f32>,
    @location(2) previous: vec4<f32>,
    @location(3) facing: f32,
    @location(4) reach: f32,
    @location(5) uv: vec2<f32>,
    @location(6) @interpolate(flat) material: u32,
    @location(7) @interpolate(flat) fade: f32,
    @location(8) @interpolate(flat) character: u32,
}

@vertex
fn vs_outline_prepass_masked(
    packed: PackedVertex,
    @location(10) previous_position: vec4<f32>,
    draw: DrawRecord,
) -> MaskedOutlinePrepassVaryings {
    let hull = outline_prepass_varyings(packed, previous_position, draw);
    let instance = instances[draw.instance];
    var out: MaskedOutlinePrepassVaryings;
    out.clip_position = hull.clip_position;
    out.world_normal = hull.world_normal;
    out.current = hull.current;
    out.previous = hull.previous;
    out.facing = hull.facing;
    out.reach = hull.reach;
    out.uv = draw.uv_transform.xy + draw.uv_transform.zw * packed.uv;
    out.material = draw.material;
    out.fade = instance.fade;
    out.character = instance.character;
    return out;
}

// The hull's targets where its main pass keeps the pixel: the material's alpha test and the
// character's dither.
@fragment
fn fs_outline_prepass_masked(in: MaskedOutlinePrepassVaryings) -> OutlinePrepassTargets {
    let uv = MaterialUv(in.uv, dpdx(in.uv), dpdy(in.uv));
    let hull = OutlinePrepassVaryings(in.clip_position, in.world_normal, in.current, in.previous, in.facing, in.reach);
    let targets = outline_prepass_targets(hull);
    let material = in.material;
    let modes = materials[material].modes;
    var alpha = materials[material].base_color.a;
    if material_has_texture(modes, CHARACTER_BASE_COLOR) {
        alpha *= sample_material(base_color_map, materials[material].swizzles, CHARACTER_BASE_COLOR, uv).a;
    }
    let cut = (modes.x & CHARACTER_FLAG_ALPHA_TEST) != 0u && alpha < materials[material].split.w;
    // A fading character's lines (M1 C7) are its own, for temporal AA's responsive mask.
    let outline = select(OUTLINE_MASK_LINE, OUTLINE_MASK_FADING_LINE, in.fade > 0.0);
    if cut || faded_out(in.fade, in.clip_position.xy, in.character, FADE_SAMPLES) {
        discard;
    }
    return OutlinePrepassTargets(targets.normal, targets.velocity, outline);
}
