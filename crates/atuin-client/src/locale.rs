//! Conventions that follow the user's POSIX locale.

use interim::Dialect;

/// Extensions to [`Dialect`].
pub trait DialectExt {
    /// The dialect of the locale that governs dates: the first non-empty of `LC_ALL`, `LC_TIME`
    /// and `LANG`.
    fn from_env() -> Self;

    /// The dialect of a POSIX locale such as `en_GB.UTF-8`.
    ///
    /// [`Dialect::Us`] if its numeric dates put the month first, which includes `C` (`%m/%d/%y`) and
    /// the empty locale that falls back to it; [`Dialect::Uk`] otherwise.
    fn from_locale(locale: &str) -> Self;
}

impl DialectExt for Dialect {
    fn from_env() -> Self {
        let locale = ["LC_ALL", "LC_TIME", "LANG"]
            .into_iter()
            .find_map(|var| std::env::var(var).ok().filter(|locale| !locale.is_empty()))
            .unwrap_or_default();
        Self::from_locale(&locale)
    }

    fn from_locale(locale: &str) -> Self {
        /// Every glibc and macOS locale whose CLDR 48 month-day pattern puts the month first, as
        /// `1/2` (`en_US`) and `1. 2.` (`hu_HU`) do for January 2nd.
        const MONTH_FIRST: &[&str] = &[
            "ak_GH", "bem_ZM", "bho_IN", "bho_NP", "bo_CN", "bo_IN", "brx_IN", "ce_RU", "chr_US",
            "ckb_IQ", "cmn_TW", "cv_RU", "dz_BT", "en_CA", "en_PH", "en_US", "en_ZA", "es_PA",
            "es_PR", "eu_ES", "fa_AF", "fa_IR", "fil_PH", "fr_CA", "gv_GB", "ha_NG", "hu_HU",
            "ja_JP", "kl_GL", "ko_KR", "ks_IN", "kw_GB", "lg_UG", "lij_IT", "lt_LT", "mn_MN",
            "mt_MT", "nds_DE", "nds_NL", "ne_NP", "nso_ZA", "oc_FR", "om_ET", "om_KE", "or_IN",
            "ps_AF", "quz_PE", "raj_IN", "sah_RU", "sat_IN", "sd_IN", "se_NO", "shn_MM", "si_LK",
            "so_DJ", "so_ET", "so_KE", "so_SO", "st_ZA", "szl_PL", "tl_PH", "tn_ZA", "xh_ZA",
            "yi_US", "yue_HK", "zh_CN", "zh_SG", "zh_TW", "zu_ZA",
        ];

        let name = locale.split(['.', '@']).next().unwrap_or_default();
        if matches!(name, "" | "C" | "POSIX") || MONTH_FIRST.contains(&name) {
            Self::Us
        } else {
            Self::Uk
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::us("en_US.UTF-8", Dialect::Us)]
    #[case::uk("en_GB.UTF-8", Dialect::Uk)]
    #[case::germany("de_DE.UTF-8", Dialect::Uk)]
    #[case::spanish_in_the_us("es_US.UTF-8", Dialect::Uk)]
    #[case::year_first("ja_JP.UTF-8", Dialect::Us)]
    #[case::year_first_but_day_before_month("sv_SE.UTF-8", Dialect::Uk)]
    #[case::modifier("sr_RS@latin", Dialect::Uk)]
    #[case::c("C.UTF-8", Dialect::Us)]
    #[case::posix("POSIX", Dialect::Us)]
    #[case::unset("", Dialect::Us)]
    fn from_locale(#[case] locale: &str, #[case] expected: Dialect) {
        assert_eq!(Dialect::from_locale(locale), expected);
    }
}
