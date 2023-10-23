//! Sensor Independent Complex Data support
//!
//! The primary interface for general sicd reading is `read_sicd`.
//!
//! It is a future goal to have functions for each version, but for now a single
//! function call and `match` statement are used.
use std::fs::File;
use std::path::Path;
use std::slice::from_raw_parts;
use std::str::{from_utf8, FromStr};

use ndarray::{Array2, ArrayView2, par_azip};

use thiserror::Error;
use serde::Deserialize;
use num_complex::Complex;
use quick_xml::de::from_str;

use nitf_rs::Nitf;
use nitf_rs::segments::NitfSegment;
use nitf_rs::headers::ImageHeader;

pub mod dep;
pub mod v1_3_0;

#[derive(Error, Debug)]
pub enum SicdError {
    /// "unknown sicd version {}"
    #[error("unknown sicd version {0}")]
    VersionError(String),
    /// "metadata for version {} is not implemented"
    #[error("metadata for version {0} is not implemented")]
    Unimpl(String),
}

/// SICD file structure
#[derive(Debug)]
pub struct Sicd {
    /// Nitf file object and associated metadata
    pub nitf: Nitf,
    /// Parsed SICD xml metadata
    pub meta: SicdMeta,
    /// SICD Version
    pub version: SicdVersion,
    /// Image data from Nitf Image segements
    pub image_data: Vec<ImageData>,
    file: &'static File
}

/// Image data structure. Currently only implements Complex<f32> data type
#[derive(Debug, Default)]
pub struct ImageData {
    byte_slice: &'static [u8],
    new_size: usize,
    pub array: Array2<Complex<f32>>,
    pub n_rows: usize,
    pub n_cols: usize,
}
impl ImageData {
    pub fn initialize(slice: &'static [u8], meta: &ImageHeader) -> Self {
        let mut im_data = Self::default();
        im_data.byte_slice = slice;
        im_data.n_rows = meta.nrows.val as usize;
        im_data.n_cols = meta.ncols.val as usize;
        im_data.new_size = im_data.byte_slice.len() / std::mem::size_of::<Complex<f32>>();
        let f32_ptr = im_data.byte_slice.as_ptr() as *const [[u8; 4]; 2]; // bit layout of complex number
        let float_slice = unsafe {
            from_raw_parts(f32_ptr, im_data.new_size)
        };
        let aview = ArrayView2::from_shape(
            (im_data.n_rows, im_data.n_cols),
            float_slice
        ).unwrap();
        im_data.array = Array2::zeros((im_data.n_rows, im_data.n_cols));

        par_azip!((out_elem in &mut im_data.array, in_elem in &aview) {
            out_elem.re = f32::from_be_bytes(in_elem[0]);
            out_elem.im = f32::from_be_bytes(in_elem[1]);
        });
        im_data
    }
}

#[derive(Debug, Deserialize, PartialEq)]
pub enum SicdVersion {
    V0_3_1,
    V0_4_0,
    V0_4_1,
    V0_5_0,
    V1_0_0,
    V1_0_1,
    V1_1_0,
    V1_2_0,
    V1_2_1,
    V1_3_0,
}

impl FromStr for SicdVersion {
    type Err = SicdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split("urn:SICD:").collect::<String>().as_str() {
            "0.3.1" => Ok(SicdVersion::V0_3_1),
            "0.4.0" => Ok(SicdVersion::V0_4_0),
            "0.4.1" => Ok(SicdVersion::V0_4_1),
            "0.5.0" => Ok(SicdVersion::V0_5_0),
            "1.0.0" => Ok(SicdVersion::V1_0_0),
            "1.0.1" => Ok(SicdVersion::V1_0_1),
            "1.1.0" => Ok(SicdVersion::V1_1_0),
            "1.2.0" => Ok(SicdVersion::V1_2_0),
            "1.2.1" => Ok(SicdVersion::V1_2_1),
            "1.3.0" => Ok(SicdVersion::V1_3_0),
            _ => Err(SicdError::VersionError(s.to_string())),
        }
    }
}

#[derive(Debug)]
pub enum SicdMeta {
    V0_3_1,  // Not implemented
    V0_4_0(dep::v0_4_0::SicdMeta),
    V0_4_1,  // Not implemented
    V0_5_0(dep::v0_5_0::SicdMeta),
    V1(v1_3_0::SicdMeta),
}

impl SicdMeta {
    pub fn get_v0_3_1_meta(self) -> SicdError {
        SicdError::Unimpl("0.3.1".to_string())
    }
    pub fn get_v0_4_0_meta(self) -> Option<dep::v0_4_0::SicdMeta> {
        match self {
            Self::V0_4_0(meta) => Some(meta),
            _ => None
        }
    }
    pub fn get_v0_4_1_meta(self) -> SicdError {
        SicdError::Unimpl("0.4.1".to_string())
    }
    pub fn get_v0_5_0_meta(self) -> Option<dep::v0_5_0::SicdMeta> {
        match self {
            Self::V0_5_0(meta) => Some(meta),
            _ => None
        }
    }
    pub fn get_v1_meta(self) -> Option<v1_3_0::SicdMeta> {
        match self {
            Self::V1(meta) => Some(meta),
            _ => None
        }
    }
}

/// Construct a [Sicd] object from a file `path`.
///
/// This is specifically for cases where the version of the Sicd is not known
/// and makes use of several `enums` to parse the data.
///
/// # Example
/// ```
/// use std::path::Path;
/// use sicd_rs::SicdVersion;
///
/// let sicd_path = Path::new("../example.nitf");
/// let sicd = sicd_rs::read_sicd(sicd_path);
/// // Then use convenience methods provided by SicdMeta enum, or match off of version
/// let meta = sicd.meta.get_v1_meta();
/// ```
///
pub fn read_sicd(path: &Path) -> Sicd {
    let mut file = File::open(path).unwrap();
    Sicd::from_file(&mut file)
}

impl Sicd {
    pub fn from_file(file: &'static mut File) -> Self {
        let nitf = Nitf::from_file(file);
        let sicd_str = from_utf8(&nitf.data_extension_segments[0].data[..]).unwrap();
        let (version, meta) = parse_sicd(sicd_str).unwrap();
        let n_img = nitf.nitf_header.meta.numi.val as usize;
        let image_data: Vec<ImageData> = vec![];
        for i_img in 0..n_img {
            let tmp = ImageData::initialize(
                &nitf.image_segments[i_img].data,
                &nitf.image_segments[i_img].meta,
            );

        }
        Self {
            nitf,
            meta,
            version,
            image_data,
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Clone)]
struct VersionGetter {
    #[serde(rename = "@xmlns")]
    pub version: String,
}

fn parse_sicd(sicd_str: &str) -> Result<(SicdVersion, SicdMeta), SicdError> {
    // This feels bad
    let tmp: VersionGetter = from_str(&sicd_str).unwrap();
    let sicd_version = SicdVersion::from_str(&tmp.version).unwrap();
    use SicdError::Unimpl;
    match sicd_version {
        SicdVersion::V0_3_1 => Err(Unimpl("V0_3_1".to_string())),
        SicdVersion::V0_4_0 => Ok((
            SicdVersion::V0_4_0,
            SicdMeta::V0_4_0(from_str(sicd_str).unwrap()),
        )),
        SicdVersion::V0_4_1 => Err(Unimpl("V0_4_1".to_string())),
        SicdVersion::V0_5_0 => Ok((
            SicdVersion::V0_5_0,
            SicdMeta::V0_5_0(from_str(sicd_str).unwrap()),
        )),
        // Don't need to worry about anything else, all versions past 1.0 are backwards compatible
        other => Ok((other, SicdMeta::V1(from_str(sicd_str).unwrap()))),
    }
}
