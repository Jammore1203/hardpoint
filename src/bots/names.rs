//! Bot callsigns. Original, and deliberately in the same register as the
//! rest of the game's naming.

pub const CALLSIGNS: [&str; 48] = [
    "VOSS", "HALLORAN", "PIKE", "MARLOWE", "DRAY", "KESTREL", "OKONKWO", "SABLE",
    "REYES", "TALBOT", "IVANOV", "NKEMDI", "FARROW", "QUINN", "BRANNIGAN", "STRAND",
    "MERRICK", "AZUMI", "COLE", "DEVRIES", "HOLT", "SORENSEN", "BAPTISTE", "WREN",
    "KOVAC", "ELLIS", "NAKASHIMA", "DUFRESNE", "ASHER", "MBEKI", "LARKIN", "VANCE",
    "SILVA", "HARLOW", "OSTROV", "KANE", "BENEDICT", "TAMURA", "ROURKE", "FINCH",
    "ADEYEMI", "GRANT", "LINDQVIST", "MAZUR", "PRICE", "SHEPPARD", "TORRES", "YUEN",
];

/// A callsign that is not already in use on the server.
pub fn pick_unique(taken: &[String], rng: &mut crate::core::Rng) -> String {
    let start = rng.below(CALLSIGNS.len() as u32) as usize;
    for i in 0..CALLSIGNS.len() {
        let name = CALLSIGNS[(start + i) % CALLSIGNS.len()];
        if !taken.iter().any(|t| t == name) {
            return name.to_string();
        }
    }
    format!("{}-{}", CALLSIGNS[start], rng.below(99))
}
