//! Unicode 规范化。生产 `String.prototype.normalize` 只走这条路径。

use icu_normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};

/// ECMA-262 `String.prototype.normalize` 允许的 form。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NormalizationForm {
    Nfc,
    Nfd,
    Nfkc,
    Nfkd,
}

impl NormalizationForm {
    pub fn parse(form: &str) -> Result<Self, &'static str> {
        match form {
            "NFC" => Ok(Self::Nfc),
            "NFD" => Ok(Self::Nfd),
            "NFKC" => Ok(Self::Nfkc),
            "NFKD" => Ok(Self::Nfkd),
            _ => Err("The normalization form should be one of NFC, NFD, NFKC, NFKD"),
        }
    }
}

pub fn normalize(text: &str, form: NormalizationForm) -> String {
    match form {
        NormalizationForm::Nfc => ComposingNormalizerBorrowed::new_nfc()
            .normalize(text)
            .into_owned(),
        NormalizationForm::Nfd => DecomposingNormalizerBorrowed::new_nfd()
            .normalize(text)
            .into_owned(),
        NormalizationForm::Nfkc => ComposingNormalizerBorrowed::new_nfkc()
            .normalize(text)
            .into_owned(),
        NormalizationForm::Nfkd => DecomposingNormalizerBorrowed::new_nfkd()
            .normalize(text)
            .into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{NormalizationForm, normalize};

    #[test]
    fn nfc_composes_e_acute() {
        let nfd = "e\u{0301}";
        let nfc = normalize(nfd, NormalizationForm::Nfc);
        assert_eq!(nfc, "\u{00e9}");
    }

    #[test]
    fn nfkc_compat_ligature() {
        assert_eq!(normalize("\u{fb00}", NormalizationForm::Nfkc), "ff");
    }
}
