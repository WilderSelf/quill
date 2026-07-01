//! ICC handling for the PDF/X OutputIntent (spec 0002 reqs 4-5).
//!
//! Two narrow jobs: validate that a user-supplied `--icc` is a CMYK output-class profile
//! ([`check_icc`], the `IccProfileInvalid` preflight check), and synthesize a minimal CMYK
//! output profile for tests/CI so no licensed vendor profile has to be bundled
//! ([`synth_cmyk_profile`]). Content colors are already CMYK/gray, so no colour *conversion*
//! happens here.

use lcms2::{
    ColorSpaceSignature, Locale, Profile, ProfileClassSignature, Tag, TagSignature, CIEXYZ, MLU,
};

/// Validate ICC bytes as a CMYK, output-class ("printer", `prtr`) profile.
///
/// Returns a human-readable error describing the first failure. This is the body of the
/// `CheckId::IccProfileInvalid` preflight check.
pub fn check_icc(bytes: &[u8]) -> Result<(), String> {
    let profile = Profile::new_icc(bytes).map_err(|_| "not a valid ICC profile".to_string())?;
    if profile.color_space() != ColorSpaceSignature::CmykData {
        return Err("output-intent ICC must be a CMYK profile".into());
    }
    if profile.device_class() != ProfileClassSignature::OutputClass {
        return Err("output-intent ICC must be an output (printer) class profile".into());
    }
    Ok(())
}

/// Synthesize a minimal but structurally valid CMYK output-class ICC profile.
///
/// Built with Little-CMS (the reference implementation) so the bytes re-parse as a valid ICC —
/// this backs CI and sample golden tests without a licensed profile or network access. The
/// colorimetry is meaningless; only structural validity (CMYK color space, output device class,
/// required text/white-point tags) matters for a PDF/X `DestOutputProfile`.
pub fn synth_cmyk_profile() -> Vec<u8> {
    let mut p = Profile::new_placeholder();
    p.set_device_class(ProfileClassSignature::OutputClass);
    p.set_color_space(ColorSpaceSignature::CmykData);
    p.set_pcs(ColorSpaceSignature::LabData);

    let mut desc = MLU::new(1);
    desc.set_text_ascii("Quill Synthetic CMYK", Locale::new("en_US"));
    p.write_tag(TagSignature::ProfileDescriptionTag, Tag::MLU(&desc));
    p.write_tag(TagSignature::CopyrightTag, Tag::MLU(&desc));

    // D50 media white point (a required tag for a well-formed profile).
    let d50 = CIEXYZ {
        X: 0.9642,
        Y: 1.0,
        Z: 0.8249,
    };
    p.write_tag(TagSignature::MediaWhitePointTag, Tag::CIEXYZ(&d50));

    p.icc().expect("serialize synthesized CMYK ICC profile")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesized_profile_is_valid_cmyk_output() {
        let bytes = synth_cmyk_profile();
        assert!(!bytes.is_empty());
        // The reference-implementation round-trip: our synthesized profile must pass the very
        // check a user profile must pass. This is the local proxy for veraPDF ICC validity.
        check_icc(&bytes).expect("synthesized CMYK profile should validate");
    }

    #[test]
    fn rgb_profile_is_rejected() {
        // An sRGB profile is RGB / display-class → must fail.
        let srgb = Profile::new_srgb().icc().expect("serialize sRGB");
        let err = check_icc(&srgb).unwrap_err();
        assert!(err.contains("CMYK"), "unexpected error: {err}");
    }

    #[test]
    fn garbage_bytes_are_rejected() {
        assert!(check_icc(b"not an icc profile at all").is_err());
    }
}
