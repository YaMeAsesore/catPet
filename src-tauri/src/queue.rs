//! Cola editable en memoria (drag&drop en el frontend).
//! Decisión: Vec + cursor porque next/prev/reorder son O(1) y 50k caben en RAM (<10MB).

use crate::db::Track;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackQueue {
    pub items: Vec<Track>,
    #[serde(default)]
    pub current: Option<usize>,
    /// Versión estructural: sube al cambiar pistas (set/push/append/remove/
    /// move/clear) y NO al cambiar solo el cursor. La UI usa esto para evitar
    /// reenviar la cola entera cuando únicamente cambió la pista actual.
    #[serde(default)]
    pub rev: u64,
}

impl TrackQueue {
    fn bump(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }
    pub fn set_list(&mut self, tracks: Vec<Track>, start: usize) {
        self.items = tracks;
        self.current = if self.items.is_empty() {
            None
        } else {
            Some(start.min(self.items.len() - 1))
        };
        self.bump();
    }
    pub fn push(&mut self, t: Track) {
        self.items.push(t);
        if self.current.is_none() {
            self.current = Some(0);
        }
        self.bump();
    }
    /// Añade varias pistas al final sin mover el cursor (paginación de
    /// YouTube: al revelar otro lote se amplía la cola sin reiniciar nada).
    /// Ignora paths que ya estén en la cola: así un clic que reenvía la lista
    /// mientras el append está en vuelo no duplica las filas nuevas.
    pub fn append(&mut self, mut tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        let mut seen: std::collections::HashSet<String> =
            self.items.iter().map(|t| t.path.clone()).collect();
        tracks.retain(|t| seen.insert(t.path.clone()));
        if tracks.is_empty() {
            return;
        }
        self.items.append(&mut tracks);
        if self.current.is_none() {
            self.current = Some(0);
        }
        self.bump();
    }
    pub fn remove(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        self.items.remove(index);
        self.current = match self.current {
            None => None,
            Some(_) if self.items.is_empty() => None,
            Some(c) if index < c => Some(c - 1),
            Some(c) if index == c => Some(c.min(self.items.len() - 1)),
            Some(c) => Some(c),
        };
        self.bump();
    }
    /// Reordenar por drag&drop.
    pub fn move_item(&mut self, from: usize, to: usize) {
        if from >= self.items.len() || to >= self.items.len() || from == to {
            return;
        }
        let cur_track = self.current.and_then(|c| self.items.get(c).cloned());
        let item = self.items.remove(from);
        self.items.insert(to, item);
        // Recalcula el cursor siguiendo al track actual, no al índice.
        self.current = cur_track.and_then(|ct| self.items.iter().position(|t| t.id == ct.id && t.path == ct.path));
        self.bump();
    }
    pub fn clear(&mut self) {
        if self.items.is_empty() && self.current.is_none() {
            return;
        }
        self.items.clear();
        self.current = None;
        self.bump();
    }
    pub fn current_track(&self) -> Option<Track> {
        self.current.and_then(|c| self.items.get(c).cloned())
    }
    pub fn advance(&mut self, step: i32) -> Option<Track> {
        if self.items.is_empty() {
            return None;
        }
        let len = self.items.len() as i32;
        let next = match self.current {
            Some(c) => (c as i32 + step).rem_euclid(len) as usize,
            None => 0,
        };
        self.current = Some(next);
        self.current_track()
    }

    /// Avanza solo si NO es el último (auto-avance al terminar: la última se
    /// detiene en vez de envolver como el siguiente manual, que es circular).
    pub fn advance_if_not_last(&mut self) -> Option<Track> {
        let c = self.current?;
        if c + 1 >= self.items.len() {
            return None;
        }
        self.current = Some(c + 1);
        self.current_track()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(id: i64, path: &str) -> Track {
        Track {
            id, path: path.into(), title: format!("t{id}"), artist: "".into(),
            album: "".into(), genre: "".into(), year: 0, track_no: 0,
            duration_secs: 0, mtime: 0, art_url: "".into(),
        }
    }

    #[test]
    fn set_y_avance_circular() {
        let mut q = TrackQueue::default();
        q.set_list(vec![mk(1, "/a"), mk(2, "/b"), mk(3, "/c")], 0);
        assert_eq!(q.current_track().unwrap().id, 1);
        q.advance(1);
        assert_eq!(q.current_track().unwrap().id, 2);
        q.advance(2); // 1 -> 2 -> (2+2)%3=1
        assert_eq!(q.current_track().unwrap().id, 1);
        q.advance(-1); // circular hacia atrás
        assert_eq!(q.current_track().unwrap().id, 3);
    }

    #[test]
    fn quitar_ajusta_cursor() {
        let mut q = TrackQueue::default();
        q.set_list(vec![mk(1, "/a"), mk(2, "/b"), mk(3, "/c")], 1);
        q.remove(0); // antes del actual: el cursor baja
        assert_eq!(q.current, Some(0));
        assert_eq!(q.current_track().unwrap().id, 2);
        q.remove(0); // quita el actual: apunta al siguiente
        assert_eq!(q.current_track().unwrap().id, 3);
        q.remove(5); // fuera de rango: no hace nada
        assert_eq!(q.items.len(), 1);
    }

    #[test]
    fn mover_sigue_al_tema_actual() {
        let mut q = TrackQueue::default();
        q.set_list(vec![mk(1, "/a"), mk(2, "/b"), mk(3, "/c")], 1);
        q.move_item(0, 2); // /b era índice 1, ahora 0
        assert_eq!(q.current_track().unwrap().id, 2);
        assert_eq!(q.current, Some(0));
    }

    #[test]
    fn persiste_y_restaura_cola_completa() {
        let mut q = TrackQueue::default();
        q.set_list(vec![mk(1, "/a"), mk(2, "/b"), mk(3, "/c")], 1);
        let raw = serde_json::to_string(&q).unwrap();
        let back: TrackQueue = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.items.len(), 3);
        assert_eq!(back.current, Some(1));
        assert_eq!(back.rev, q.rev);
        assert_eq!(back.current_track().unwrap().path, "/b");
    }

    #[test]
    fn rev_solo_sube_cuando_cambia_la_estructura() {
        let mut q = TrackQueue::default();
        q.set_list(vec![mk(1, "/a"), mk(2, "/b")], 0);
        let r = q.rev;
        q.advance(1); // solo cursor
        assert_eq!(q.rev, r);
        q.move_item(0, 1);
        assert!(q.rev > r);
        let r2 = q.rev;
        q.remove(0);
        assert!(q.rev > r2);
        let r3 = q.rev;
        q.clear();
        assert!(q.rev > r3);
        // Vaciar una cola ya vacía no genera una revisión inútil.
        let r4 = q.rev;
        q.clear();
        assert_eq!(q.rev, r4);
    }

    #[test]
    fn append_conserva_cursor() {
        let mut q = TrackQueue::default();
        q.set_list(vec![mk(1, "/a"), mk(2, "/b")], 1);
        q.append(vec![mk(3, "/c"), mk(4, "/d")]);
        assert_eq!(q.items.len(), 4);
        assert_eq!(q.current, Some(1));
        assert_eq!(q.current_track().unwrap().id, 2);
        // Repetir el mismo append no duplica (carrera clic/append).
        let rev = q.rev;
        q.append(vec![mk(3, "/c"), mk(4, "/d")]);
        assert_eq!(q.items.len(), 4);
        assert_eq!(q.rev, rev);
        // En una cola vacía, el primer añadido pasa a ser el actual.
        let mut empty = TrackQueue::default();
        empty.append(vec![mk(9, "/z")]);
        assert_eq!(empty.current, Some(0));
    }

    #[test]
    fn vacia_no_avanza() {
        let mut q = TrackQueue::default();
        assert!(q.advance(1).is_none());
        q.clear();
        assert!(q.current.is_none());
    }

    #[test]
    fn auto_avance_no_envuelve_al_final() {
        let mut q = TrackQueue::default();
        q.set_list(vec![mk(1, "/a"), mk(2, "/b"), mk(3, "/c")], 0);
        // En medio: avanza y mueve el cursor.
        assert_eq!(q.advance_if_not_last().unwrap().id, 2);
        assert_eq!(q.current, Some(1));
        assert_eq!(q.advance_if_not_last().unwrap().id, 3);
        // En la última: None Y el cursor no se mueve (se detiene, no repite).
        assert!(q.advance_if_not_last().is_none());
        assert_eq!(q.current, Some(2));
        assert!(q.advance_if_not_last().is_none());
        // Vacía o sin cursor: None.
        let mut empty = TrackQueue::default();
        assert!(empty.advance_if_not_last().is_none());
    }
}
