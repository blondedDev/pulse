use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, ElementId, Hsla, Image, IntoElement, ObjectFit,
    RenderImage, Styled, StyledImage, ease_out_quint, img, px,
};

use crate::cover_placeholder::cover_placeholder;

pub fn cover_thumb(
    cover: Option<&Arc<Image>>,
    size: f32,
    radius: f32,
    bg: Hsla,
    fg: Hsla,
) -> AnyElement {
    match cover {
        Some(c) => img(c.clone())
            .w(px(size))
            .h(px(size))
            .rounded(px(radius))
            .object_fit(ObjectFit::Cover)
            .with_fallback(move || cover_placeholder(size, radius, bg, fg).into_any_element())
            .into_any_element(),
        None => cover_placeholder(size, radius, bg, fg).into_any_element(),
    }
}

/// [`cover_thumb`], faded in when it arrives.
///
/// `fade` is supplied by the caller (which is the layer that can read the
/// animation settings) and is only applied once there *is* a cover: the
/// placeholder is not animated, so the fade reads as the art settling in rather
/// than the box appearing. Give it an id that changes when the cover arrives
/// (see `cover_fade_id`), or the animation will never re-trigger.
pub fn cover_thumb_fading(
    cover: Option<&Arc<Image>>,
    size: f32,
    radius: f32,
    bg: Hsla,
    fg: Hsla,
    id: ElementId,
    fade: Option<Duration>,
) -> AnyElement {
    // The animation has to wrap the concrete image: `AnyElement` is not
    // `Styled`, so it has no `opacity` to animate. The placeholder is returned
    // untouched — it is the thing the cover replaces, not something to fade.
    match cover {
        Some(c) => {
            let image = img(c.clone())
                .w(px(size))
                .h(px(size))
                .rounded(px(radius))
                .object_fit(ObjectFit::Cover)
                .with_fallback(move || cover_placeholder(size, radius, bg, fg).into_any_element());
            match fade {
                Some(ms) => image
                    .with_animation(
                        id,
                        Animation::new(ms).with_easing(ease_out_quint()),
                        |el, delta| el.opacity(delta),
                    )
                    .into_any_element(),
                None => image.into_any_element(),
            }
        }
        None => cover_placeholder(size, radius, bg, fg).into_any_element(),
    }
}

/// The id `cover_thumb_fading` needs: one value per (cover, loaded) pair, so
/// the animation runs exactly when the picture replaces the placeholder.
pub fn cover_fade_id(cover_art_id: Option<i64>, loaded: bool) -> ElementId {
    ElementId::NamedInteger(
        "cover-fade".into(),
        (cover_art_id.unwrap_or(0) as u64) * 2 + u64::from(loaded),
    )
}

pub fn cover_tile(
    cover: Option<&Arc<RenderImage>>,
    size: f32,
    radius: f32,
    bg: Hsla,
    fg: Hsla,
) -> AnyElement {
    match cover {
        Some(c) => img(c.clone())
            .w(px(size))
            .h(px(size))
            .rounded(px(radius))
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        None => cover_placeholder(size, radius, bg, fg).into_any_element(),
    }
}
