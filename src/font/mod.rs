//! Font handling.

mod book;
mod variant;

pub use self::book::*;
pub use self::variant::*;

use std::fmt::{self, Debug, Formatter};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use once_cell::unsync::OnceCell;
use rex::font::MathHeader;
use ttf_parser::{GlyphId, Tag};

use crate::geom::Em;
use crate::util::Buffer;

/// An OpenType font.
#[derive(Clone)]
pub struct Font(Arc<Repr>);

/// The internal representation of a font.
struct Repr {
    /// The raw font data, possibly shared with other fonts from the same
    /// collection. The vector's allocation must not move, because `ttf` points
    /// into it using unsafe code.
    data: Buffer,
    /// The font's index in the buffer.
    index: u32,
    /// Metadata about the font.
    info: FontInfo,
    /// The font's metrics.
    metrics: FontMetrics,
    /// The underlying ttf-parser face.
    ttf: ttf_parser::Face<'static>,
    /// The underlying rustybuzz face.
    rusty: rustybuzz::Face<'static>,
    /// The parsed ReX math header.
    math: OnceCell<Option<MathHeader>>,
}

impl Font {
    /// Parse a font from data and collection index.
    pub fn new(data: Buffer, index: u32) -> Option<Self> {
        // Safety:
        // - The slices's location is stable in memory:
        //   - We don't move the underlying vector
        //   - Nobody else can move it since we have a strong ref to the `Arc`.
        // - The internal 'static lifetime is not leaked because its rewritten
        //   to the self-lifetime in `ttf()`.
        let slice: &'static [u8] =
            unsafe { std::slice::from_raw_parts(data.as_ptr(), data.len()) };

        let ttf = ttf_parser::Face::parse(slice, index).ok()?;
        let rusty = rustybuzz::Face::from_slice(slice, index)?;
        let metrics = FontMetrics::from_ttf(&ttf);
        let info = FontInfo::from_ttf(&ttf)?;

        Some(Self(Arc::new(Repr {
            data,
            index,
            info,
            metrics,
            ttf,
            rusty,
            math: OnceCell::new(),
        })))
    }

    /// The underlying buffer.
    pub fn data(&self) -> &Buffer {
        &self.0.data
    }

    /// The font's index in the buffer.
    pub fn index(&self) -> u32 {
        self.0.index
    }

    /// The font's metadata.
    pub fn info(&self) -> &FontInfo {
        &self.0.info
    }

    /// The font's metrics.
    pub fn metrics(&self) -> &FontMetrics {
        &self.0.metrics
    }

    /// The number of font units per one em.
    pub fn units_per_em(&self) -> f64 {
        self.0.metrics.units_per_em
    }

    /// Convert from font units to an em length.
    pub fn to_em(&self, units: impl Into<f64>) -> Em {
        Em::from_units(units, self.units_per_em())
    }

    /// Look up the horizontal advance width of a glyph.
    pub fn advance(&self, glyph: u16) -> Option<Em> {
        self.0
            .ttf
            .glyph_hor_advance(GlyphId(glyph))
            .map(|units| self.to_em(units))
    }

    /// Lookup a name by id.
    pub fn find_name(&self, id: u16) -> Option<String> {
        find_name(&self.0.ttf, id)
    }

    /// A reference to the underlying `ttf-parser` face.
    pub fn ttf(&self) -> &ttf_parser::Face<'_> {
        // We can't implement Deref because that would leak the
        // internal 'static lifetime.
        &self.0.ttf
    }

    /// A reference to the underlying `rustybuzz` face.
    pub fn rusty(&self) -> &rustybuzz::Face<'_> {
        // We can't implement Deref because that would leak the
        // internal 'static lifetime.
        &self.0.rusty
    }

    /// Access the math header, if any.
    pub fn math(&self) -> Option<&MathHeader> {
        self.0
            .math
            .get_or_init(|| {
                let data = self.ttf().raw_face().table(Tag::from_bytes(b"MATH"))?;
                MathHeader::parse(data).ok()
            })
            .as_ref()
    }
}

impl Hash for Font {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.data.hash(state);
    }
}

impl Debug for Font {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Font({})", self.info().family)
    }
}

impl Eq for Font {}

impl PartialEq for Font {
    fn eq(&self, other: &Self) -> bool {
        self.0.data.eq(&other.0.data)
    }
}

/// Metrics of a font.
#[derive(Debug, Copy, Clone)]
pub struct FontMetrics {
    /// How many font units represent one em unit.
    pub units_per_em: f64,
    /// The distance from the baseline to the typographic ascender.
    pub ascender: Em,
    /// The approximate height of uppercase letters.
    pub cap_height: Em,
    /// The approximate height of non-ascending lowercase letters.
    pub x_height: Em,
    /// The distance from the baseline to the typographic descender.
    pub descender: Em,
    /// Recommended metrics for a strikethrough line.
    pub strikethrough: LineMetrics,
    /// Recommended metrics for an underline.
    pub underline: LineMetrics,
    /// Recommended metrics for an overline.
    pub overline: LineMetrics,
}

impl FontMetrics {
    /// Extract the font's metrics.
    pub fn from_ttf(ttf: &ttf_parser::Face) -> Self {
        let units_per_em = f64::from(ttf.units_per_em());
        let to_em = |units| Em::from_units(units, units_per_em);

        let ascender = to_em(ttf.typographic_ascender().unwrap_or(ttf.ascender()));
        let cap_height = ttf.capital_height().filter(|&h| h > 0).map_or(ascender, to_em);
        let x_height = ttf.x_height().filter(|&h| h > 0).map_or(ascender, to_em);
        let descender = to_em(ttf.typographic_descender().unwrap_or(ttf.descender()));
        let strikeout = ttf.strikeout_metrics();
        let underline = ttf.underline_metrics();

        let strikethrough = LineMetrics {
            position: strikeout.map_or(Em::new(0.25), |s| to_em(s.position)),
            thickness: strikeout
                .or(underline)
                .map_or(Em::new(0.06), |s| to_em(s.thickness)),
        };

        let underline = LineMetrics {
            position: underline.map_or(Em::new(-0.2), |s| to_em(s.position)),
            thickness: underline
                .or(strikeout)
                .map_or(Em::new(0.06), |s| to_em(s.thickness)),
        };

        let overline = LineMetrics {
            position: cap_height + Em::new(0.1),
            thickness: underline.thickness,
        };

        Self {
            units_per_em,
            ascender,
            cap_height,
            x_height,
            descender,
            strikethrough,
            underline,
            overline,
        }
    }

    /// Look up a vertical metric.
    pub fn vertical(&self, metric: VerticalFontMetric) -> Em {
        match metric {
            VerticalFontMetric::Ascender => self.ascender,
            VerticalFontMetric::CapHeight => self.cap_height,
            VerticalFontMetric::XHeight => self.x_height,
            VerticalFontMetric::Baseline => Em::zero(),
            VerticalFontMetric::Descender => self.descender,
        }
    }
}

/// Metrics for a decorative line.
#[derive(Debug, Copy, Clone)]
pub struct LineMetrics {
    /// The vertical offset of the line from the baseline. Positive goes
    /// upwards, negative downwards.
    pub position: Em,
    /// The thickness of the line.
    pub thickness: Em,
}

/// Identifies a vertical metric of a font.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum VerticalFontMetric {
    /// The distance from the baseline to the typographic ascender.
    ///
    /// Corresponds to the typographic ascender from the `OS/2` table if present
    /// and falls back to the ascender from the `hhea` table otherwise.
    Ascender,
    /// The approximate height of uppercase letters.
    CapHeight,
    /// The approximate height of non-ascending lowercase letters.
    XHeight,
    /// The baseline on which the letters rest.
    Baseline,
    /// The distance from the baseline to the typographic descender.
    ///
    /// Corresponds to the typographic descender from the `OS/2` table if
    /// present and falls back to the descender from the `hhea` table otherwise.
    Descender,
}
