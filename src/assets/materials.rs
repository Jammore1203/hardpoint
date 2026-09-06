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
    Mesh,
}

pub const MAT_COUNT: usize = 64;

impl Mat {
    #[inline(always)]
    pub fn layer(self) -> u32 { self as u32 }

    pub fn from_index(i: u8) -> Mat {
        // Safe because the enum is dense from 0..MAT_COUNT and we clamp.
        let i = if (i as usize) < MAT_COUNT { i } else { 0 };
        unsafe { std::mem::transmute::<u8, Mat>(i) }
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
            | Barrel | BarrelRust | Duct | Mesh | RoofMetal => Surface::Metal,
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
            Mesh => [104, 108, 110],
        }
    }

    /// Whether the material should render with alpha testing (foliage, mesh
    /// fences, gratings). Kept to a handful of materials so the opaque pass
    /// stays the fast path.
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
