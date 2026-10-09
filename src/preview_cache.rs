use crate::backend::Project;
use gpui::{RenderImage, Window};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

const THUMBNAIL_BUDGET: usize = 32 * 1024 * 1024;
const THUMBNAIL_EDGE: u32 = 640;
const DETAIL_EDGE: u32 = 1600;
const MAX_DECODERS: usize = 2;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PreviewSource {
    path: PathBuf,
    stamp: u64,
}

impl PreviewSource {
    pub fn new(cache: &str, project: &Project) -> Option<Self> {
        if project.preview.is_none()
            || project.id.len() != 16
            || !project.id.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return None;
        }
        Some(Self {
            path: PathBuf::from(cache).join(format!("{}.jpg", project.id)),
            stamp: project.captured_at?,
        })
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PreviewKey {
    source: PreviewSource,
    edge: u32,
    detail: bool,
}

pub struct Preview {
    pub image: Arc<RenderImage>,
    pub aspect: f32,
    bytes: usize,
    used: u64,
}

pub struct PreviewCache {
    images: HashMap<PreviewKey, Preview>,
    wanted: Vec<PreviewKey>,
    loading: HashSet<PreviewKey>,
    failed: HashSet<PreviewKey>,
    edge: u32,
    clock: u64,
}

impl Default for PreviewCache {
    fn default() -> Self {
        Self {
            images: HashMap::new(),
            wanted: Vec::new(),
            loading: HashSet::new(),
            failed: HashSet::new(),
            edge: THUMBNAIL_EDGE,
            clock: 0,
        }
    }
}

impl PreviewCache {
    pub fn failed(&self, source: PreviewSource) -> bool {
        self.failed.contains(&PreviewKey {
            source,
            edge: self.edge,
            detail: false,
        })
    }

    pub fn get(&self, source: PreviewSource, detail: bool) -> Option<&Preview> {
        if detail {
            if let Some(image) = self.images.get(&PreviewKey {
                source: source.clone(),
                edge: DETAIL_EDGE,
                detail: true,
            }) {
                return Some(image);
            }
        }
        self.images.get(&PreviewKey {
            source,
            edge: self.edge,
            detail: false,
        })
    }

    pub fn request(
        &mut self,
        mut sources: Vec<PreviewSource>,
        detail: Option<PreviewSource>,
        window: &mut Window,
    ) -> bool {
        if let Some(source) = &detail {
            sources.insert(0, source.clone());
        }
        let mut unique = HashSet::new();
        sources.retain(|source| unique.insert(source.clone()));
        // Keep even a very tall viewport within the decoded thumbnail budget.
        let edge = ((THUMBNAIL_BUDGET / (4 * sources.len().max(1))) as f64)
            .sqrt()
            .floor() as u32;
        // Do not oscillate between sizes as a partial row enters/leaves the viewport.
        let edge = if edge < self.edge {
            (edge / 64 * 64).max(1)
        } else {
            self.edge
        };
        let mut wanted = Vec::with_capacity(sources.len() + 1);
        if let Some(source) = detail {
            wanted.push(PreviewKey {
                source,
                edge: DETAIL_EDGE,
                detail: true,
            });
        }
        wanted.extend(sources.into_iter().map(|source| PreviewKey {
            source,
            edge,
            detail: false,
        }));
        if self.wanted == wanted {
            return false;
        }
        let resized = self.edge != edge;
        self.edge = edge;
        self.wanted = wanted;
        self.clock += 1;
        self.failed.retain(|key| self.wanted.contains(key));
        for key in &self.wanted {
            if let Some(preview) = self.images.get_mut(key) {
                preview.used = self.clock;
            }
        }
        // A detail image lives only as long as its modal; old-size thumbnails are redundant.
        let obsolete = self
            .images
            .keys()
            .filter(|key| {
                (key.detail && !self.wanted.contains(key)) || (!key.detail && key.edge != edge)
            })
            .cloned()
            .collect::<Vec<_>>();
        for key in obsolete {
            self.remove(&key, window);
        }
        self.trim(window);
        resized
    }

    pub fn next_jobs(&mut self) -> Vec<PreviewKey> {
        let mut jobs = Vec::new();
        for key in &self.wanted {
            if self.loading.len() >= MAX_DECODERS {
                break;
            }
            if !self.images.contains_key(key)
                && !self.failed.contains(key)
                && self.loading.insert(key.clone())
            {
                jobs.push(key.clone());
            }
        }
        jobs
    }

    pub fn complete(
        &mut self,
        key: PreviewKey,
        loaded: Option<Preview>,
        window: &mut Window,
    ) -> bool {
        self.loading.remove(&key);
        // Jobs already decoding may finish after a fast scroll or folder switch.
        if !self.wanted.contains(&key) {
            return false;
        }
        if let Some(mut preview) = loaded {
            preview.used = self.clock;
            self.images.insert(key, preview);
            self.trim(window);
            true
        } else {
            self.failed.insert(key);
            true
        }
    }

    fn trim(&mut self, window: &mut Window) {
        let mut bytes: usize = self
            .images
            .iter()
            .filter(|(key, _)| !key.detail)
            .map(|(_, preview)| preview.bytes)
            .sum();
        while bytes > THUMBNAIL_BUDGET {
            let oldest = self
                .images
                .iter()
                .filter(|(key, _)| !key.detail && !self.wanted.contains(key))
                .min_by_key(|(_, preview)| preview.used)
                .map(|(key, preview)| (key.clone(), preview.bytes));
            let Some((key, size)) = oldest else { break };
            self.remove(&key, window);
            bytes -= size;
        }
    }

    fn remove(&mut self, key: &PreviewKey, window: &mut Window) {
        if let Some(preview) = self.images.remove(key) {
            // Dropping our Arc alone does not evict GPUI's GPU texture atlas entry.
            let _ = window.drop_image(preview.image);
        }
    }
}

pub fn decode(key: &PreviewKey) -> Option<Preview> {
    let mut reader = image::ImageReader::open(&key.source.path).ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    let aspect = decoded.width() as f32 / decoded.height().max(1) as f32;
    let mut pixels = if decoded.width().max(decoded.height()) > key.edge {
        decoded
            .resize(key.edge, key.edge, image::imageops::FilterType::Triangle)
            .into_rgba8()
    } else {
        decoded.into_rgba8()
    };
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(Preview {
        bytes: pixels.len(),
        image: Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])),
        aspect,
        used: 0,
    })
}
