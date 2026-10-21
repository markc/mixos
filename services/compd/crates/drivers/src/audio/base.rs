// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::world::audio::controller::interface::AudioController;
use crate::world::audio::controller::interface::media::MediaController;
use slots::storage::token::base::{Token, TokenMut};

/// Audio driver data: the audio + media controllers live in the kernel/driver
/// storage (reached by token), not as Orchestrator fields. `Option` because
/// device init can fail. Mutable: systems/handlers drive the controllers.
pub static AUDIO: Token<Option<AudioController>> = Token::new();
pub static AUDIO_MUT: TokenMut<Option<AudioController>> = TokenMut::new(&AUDIO);
pub static MEDIA: Token<Option<MediaController>> = Token::new();
pub static MEDIA_MUT: TokenMut<Option<MediaController>> = TokenMut::new(&MEDIA);
