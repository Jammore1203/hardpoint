//! Material catalogue.
//!
//! Every surface in the game references one of these by index. Each becomes a
//! single layer of one texture array, so the entire world draws in one call
//! regardless of how many materials it uses, and there is no atlas bleeding at
//! low mip levels.

/// Surface material. The discriminant is the texture array layer index, so the
/// order here must match `texgen::generate_world_array`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Mat {
    // -- Structural ------------------------------------------------------
    Concrete = 0,
    ConcreteWorn,
    ConcretePanel,
    Cinderblock,
    BrickRed,
    BrickPale,
    Plaster,
    StoneWall,
    // -- Metal -----------------------------------------------------------
    MetalPanel,
    MetalRust,
    MetalPlateDiamond,
    Corrugated,
    Grating,
    HullPainted,
    PipeMetal,
    ShippingRed,
    ShippingBlue,
    ShippingGreen,
    // -- Ground ----------------------------------------------------------
    Sand,
    SandRock,
    Dirt,
    Gravel,
    Grass,
    JungleFloor,
    Snow,
    SnowRock,
    Asphalt,
    ConcreteFloor,
    TileFloor,
    WoodFloor,
    Mud,
    Cobble,
    // -- Detail ----------------------------------------------------------
    WoodCrate,
    WoodPlank,
    Sandbag,
    Camo,
    CamoDesert,
    CamoWinter,
    Tarp,
    Canvas,
    Glass,
    WindowLit,
    Screen,
    ControlPanel,
    Barrel,
    BarrelRust,
    Tire,
    // -- Trim / accents ---------------------------------------------------
    HazardStripe,
    RedPaint,
    BluePaint,
    YellowPaint,
    Sign,
    RoofTile,
    RoofMetal,
    Foliage,
    Rock,
    Ice,
    WaterSurface,
    Rubber,
    Fabric,
    DirtRoad,
    Marble,
    Bunker,
    Duct,
    // -- Weapons ----------------------------------------------------------
    // Guns were built out of the same MetalPanel and MetalRust as the crates
    // and the pipework, which made every rifle in the game a light blue-grey
    // object with orange furniture. These two exist so a weapon can be the
    // colour a weapon is.
    GunMetal,
    GunPolymer,
    // -- Distance ---------------------------------------------------------
    /// A block of flats seen from four hundred metres: a concrete field with
    /// a regular grid of windows in it. What a backdrop needs and a plain
    /// wall texture cannot give it is storeys, because storeys are how the
    /// eye reads a silhouette as a building and works out how far away it is.
    Facade,
    Mesh,
}

/// The number of materials, and therefore the layer index the shared detail
/// noise tile lives at. It has to be exactly the number of `Mat` variants:
/// one short and the last material renders as the detail tile, which is how
/// `Mesh` spent its life drawing grey noise.
pub const MAT_COUNT: usize = Mat::Mesh as usize + 1;

impl Mat {
    #[inline(always)]
    pub fn layer(self) -> u32 { self as u32 }

    pub fn from_index(i: u8) -> Mat {
        // Safe because the enum is dense from 0..MAT_COUNT and we clamp.
        let i = if (i as usize) < MAT_COUNT { i } else { 0 };
        unsafe { std::mem::transmute::<u8, Mat>(i) }
    }

    /// How sharply this surface returns a highlight, from 0 (a rag) to 1
    /// (polished glass).
    ///
    /// The world has no normal maps and no material buffer; it has flat
    /// brush faces and baked vertex light, which is exactly what the games
    /// this one is imitating had. What those games did have, and what the
    /// renderer was missing, is a specular term. Without one, wet asphalt,
    /// a pane of glass, a diamond-plate catwalk and a sandbag all return
    /// light the same way, and every surface reads as chalk. One float per
    /// material is enough to separate them.
    pub fn gloss(self) -> f32 {
        use Mat::*;
        match self {
            // Nothing at all: fibre, foliage, loose grain.
            Sandbag | Canvas | Camo | CamoDesert | CamoWinter | Tarp | Fabric
            | Foliage | Grass | JungleFloor | Sand | Dirt | DirtRoad => 0.0,
            // Masonry has a faint sheen only where it has been troweled or
            // sealed; raw block and brick have none worth drawing.
            Cinderblock | BrickRed | BrickPale | StoneWall | Rock | SandRock
            | SnowRock | Gravel | Bunker => 0.04,
            Concrete | ConcreteWorn | ConcretePanel | Plaster | ConcreteFloor
            | Cobble | RoofTile | Facade => 0.09,
            // Wood: sealed floorboards catch a window, packing crates do not.
            WoodCrate | WoodPlank => 0.10,
            WoodFloor => 0.22,
            // Trodden snow and churned mud are both wet.
            Snow => 0.18,
            Mud => 0.28,
            Asphalt => 0.16,
            // Rubber is dark and dull but not matte.
            Rubber | Tire => 0.12,
            // Painted metal and steel plate: the workhorse of the palette.
            MetalRust | BarrelRust | Corrugated => 0.20,
            RoofMetal | Duct | Mesh | Grating | ShippingRed | ShippingBlue
            | ShippingGreen | Barrel => 0.36,
            MetalPanel | HullPainted | PipeMetal | RedPaint | BluePaint
            | YellowPaint | HazardStripe | Sign => 0.48,
            // Blued steel is polished; a polymer furniture set is not.
            GunMetal => 0.44,
            GunPolymer => 0.17,
            MetalPlateDiamond | ControlPanel => 0.55,
            // The polished end.
            TileFloor | Marble => 0.70,
            Screen => 0.78,
            Ice => 0.85,
            Glass | WindowLit => 0.95,
            WaterSurface => 0.92,
        }
    }

    /// Footstep / bullet-impact sound family. Keeping this on the material
    /// means level authors get correct audio for free.
    pub fn surface(self) -> Surface {
        use Mat::*;
        match self {
            Sand | SandRock | CamoDesert => Surface::Sand,
            Dirt | Mud | JungleFloor | DirtRoad | Foliage => Surface::Dirt,
            Gravel | Cobble | Rock | StoneWall | Marble => Surface::Gravel,
            Grass => Surface::Grass,
            Snow | SnowRock | CamoWinter | Ice => Surface::Snow,
            MetalPanel | MetalRust | MetalPlateDiamond | Corrugated | Grating
            | HullPainted | PipeMetal | ShippingRed | ShippingBlue | ShippingGreen
            | Barrel | BarrelRust | Duct | Mesh | RoofMetal | GunMetal
            | GunPolymer => Surface::Metal,
            WoodFloor | WoodCrate | WoodPlank => Surface::Wood,
            Glass | WindowLit | Screen => Surface::Glass,
            Sandbag | Tarp | Canvas | Camo | Fabric | Rubber | Tire => Surface::Soft,
            WaterSurface => Surface::Water,
            _ => Surface::Concrete,
        }
    }

    /// Base albedo tint applied to the generated texture, keeping the whole
    /// game inside one deliberately limited palette.
    pub fn tint(self) -> [u8; 3] {
        use Mat::*;
        match self {
            Concrete => [148, 148, 142],
            ConcreteWorn => [128, 126, 118],
            ConcretePanel => [158, 156, 150],
            Cinderblock => [140, 138, 130],
            BrickRed => [140, 72, 56],
            BrickPale => [176, 152, 124],
            Plaster => [190, 182, 164],
            StoneWall => [132, 128, 120],
            MetalPanel => [122, 128, 134],
            MetalRust => [122, 84, 58],
            MetalPlateDiamond => [110, 116, 122],
            Corrugated => [134, 138, 140],
            Grating => [96, 100, 104],
            HullPainted => [86, 104, 116],
            PipeMetal => [128, 130, 128],
            ShippingRed => [150, 60, 48],
            ShippingBlue => [52, 84, 128],
            ShippingGreen => [62, 106, 72],
            Sand => [196, 172, 118],
            SandRock => [176, 152, 106],
            Dirt => [124, 100, 72],
            Gravel => [136, 132, 124],
            Grass => [92, 118, 62],
            JungleFloor => [72, 88, 52],
            Snow => [222, 228, 236],
            SnowRock => [176, 186, 196],
            Asphalt => [76, 76, 80],
            ConcreteFloor => [136, 136, 132],
            TileFloor => [162, 160, 152],
            WoodFloor => [138, 104, 66],
            Mud => [96, 78, 56],
            Cobble => [124, 120, 114],
            WoodCrate => [154, 116, 70],
            WoodPlank => [132, 98, 60],
            Sandbag => [154, 140, 100],
            Camo => [86, 96, 68],
            CamoDesert => [166, 148, 106],
            CamoWinter => [196, 200, 204],
            Tarp => [98, 104, 88],
            Canvas => [148, 140, 116],
            Glass => [120, 148, 156],
            WindowLit => [212, 190, 130],
            Screen => [70, 120, 130],
            ControlPanel => [88, 92, 98],
            Barrel => [96, 116, 88],
            BarrelRust => [128, 88, 62],
            Tire => [74, 74, 78],
            HazardStripe => [188, 160, 52],
            RedPaint => [148, 58, 50],
            BluePaint => [58, 88, 140],
            YellowPaint => [196, 168, 58],
            Sign => [180, 178, 170],
            RoofTile => [126, 82, 68],
            RoofMetal => [110, 114, 118],
            Foliage => [72, 100, 56],
            Rock => [122, 118, 112],
            Ice => [186, 208, 220],
            WaterSurface => [64, 100, 112],
            Rubber => [84, 84, 88],
            Fabric => [128, 116, 100],
            DirtRoad => [138, 116, 84],
            Marble => [186, 182, 176],
            Bunker => [116, 116, 108],
            Duct => [140, 142, 144],
            GunMetal => [60, 63, 70],
            GunPolymer => [40, 41, 44],
            Facade => [148, 144, 136],
            Mesh => [104, 108, 110],
        }
    }

    /// Whether the material should render with alpha testing (foliage, mesh
    /// fences, gratings). Kept to a handful of materials so the opaque pass
    /// stays the fast path.
    /// Surfaces whose texture drifts, so they are not frozen mid-ripple.
    /// Only water: ice is meant to be still.
    pub fn is_liquid(self) -> bool { matches!(self, Mat::WaterSurface) }

    pub fn is_cutout(self) -> bool {
        matches!(self, Mat::Foliage | Mat::Mesh | Mat::Grating)
    }

    /// Slightly emissive surfaces get a lighting floor so they read at night.
    pub fn emissive(self) -> f32 {
        match self {
            Mat::WindowLit => 0.55,
            Mat::Screen => 0.40,
            Mat::ControlPanel => 0.22,
            Mat::HazardStripe => 0.10,
            _ => 0.0,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Surface {
    Concrete,
    Metal,
    Wood,
    Dirt,
    Sand,
    Gravel,
    Grass,
    Snow,
    Glass,
    Soft,
    Water,
}

impl Surface {
    pub fn index(self) -> usize {
        use Surface::*;
        match self {
            Concrete => 0, Metal => 1, Wood => 2, Dirt => 3, Sand => 4,
            Gravel => 5, Grass => 6, Snow => 7, Glass => 8, Soft => 9, Water => 10,
        }
    }
    pub const COUNT: usize = 11;
}

#[cfg(test)]
mod material_tests {
    use super::*;

    /// `MAT_COUNT` is the texture array's layer count and the index the detail
    /// noise lives at, so a material past the end silently renders as the
    /// detail tile and `from_index` clamps it to Concrete.
    #[test]
    fn every_material_fits_in_the_array() {
        assert_eq!(Mat::Mesh as usize + 1, MAT_COUNT, "Mesh must be the last variant");
        for i in 0..MAT_COUNT {
            assert_eq!(Mat::from_index(i as u8) as usize, i);
        }
    }
}
