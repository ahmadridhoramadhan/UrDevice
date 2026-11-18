//! QR code matrices for the Pair screen. Both apps draw the same matrix, so
//! the desktop and Android QR codes come from one encoder.

use crate::{CoreError, Result};

/// A square QR matrix. `modules[y * size + x]` is true for a dark module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrMatrix {
    /// Modules per side, without the quiet zone.
    pub size: u32,
    /// Row-major modules.
    pub modules: Vec<bool>,
}

/// Encode `text` with medium error correction.
pub fn encode(text: &str) -> Result<QrMatrix> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
        .map_err(|e| CoreError::InvalidInput(e.to_string()))?;
    let size =
        u32::try_from(code.width()).map_err(|_| CoreError::InvalidInput("too large".into()))?;
    let modules = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Ok(QrMatrix { size, modules })
}

#[cfg(test)]
mod tests {
    #[test]
    fn encodes_a_pairing_uri() {
        let m = super::encode(&format!("urdevice://pair?d={}", "A".repeat(220))).unwrap();
        assert!(m.size >= 21);
        assert_eq!(m.modules.len(), (m.size * m.size) as usize);
    }
}
