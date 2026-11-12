# lecteur-hd60

Lecteur faible latence (image + son) pour la carte de capture USB NZXT Signal HD60, en Rust.
Cible principale : Windows 10/11 x64. Affichage GPU (wgpu), menu egui, capture nokhwa, audio cpal.

## Commandes
- Compiler : `cargo build --release` (ou `build.bat` pour un .exe autonome dans `dist\`)
- Tests : `cargo test --release -- --nocapture` (les tests GPU sont ignorés sans pilote graphique)
- Lancer : `cargo run --release -- --help`

## Architecture (src/)
- `main.rs` : boucle winit, capture (thread), audio, touches, menu, réglages à chaud
- `gpu.rs` : surface wgpu, mode de présentation (Mailbox/Fifo/Immediate), filtres Lanczos/bicubique en deux passes
- `shader.wgsl` : passes direct / horizontale / verticale (poids lus dans une texture, aucun sinus sur le GPU) et conversion YUYV -> RVB (`fs_yuyv`, couleurs interpolées ; test dans `shader_test.rs`)
- `ui.rs` : panneau clair à droite (accueil, puis réglages à onglets), mire de barres sans signal, pastilles, thème et polices
  (Segoe UI / Bahnschrift lues dans C:\Windows\Fonts, avec repli sur les polices d'egui)
- `settings.rs` : réglages enregistrés dans %APPDATA%\lecteur-hd60\reglages.txt
- `audio.rs` : entrée carte de capture -> sortie par défaut, tampon ~25 ms
- `build.rs` + `assets/` : logo (SVG, PNG, ICO) et icône de l exe ; `icon-64.rgba` est inclus dans `main.rs` pour la fenêtre
- `sys.rs` : priorité processus, minuteur 1 ms (Windows)
- `shader_test.rs` : tests de rendu sans fenêtre

## Points d'attention
- Ne pas supprimer `Cargo.lock` (versions figées).
- Le projet n'a jamais été lancé sur une vraie HD60 par l'auteur du code : valider d'abord l'ouverture de la carte,
  le son, puis la charge GPU en mode « Immédiat ».
- Deux états : accueil (`Ui::home` : aperçu sans son) et direct. `start_live` / `go_home` dans `main.rs` démarrent et
  coupent le son (la capture tourne toujours) ; `Ui::wants_input()` = panneau ou message à boutons. Le panneau (344 points)
  réserve sa largeur à l image via `Ui::tick` puis `Gfx::set_inset_right`. Le son ne démarre qu en direct (`at_home()`).
- Bug corrigé : egui répond toujours « repaint » à RedrawRequested ; ne pas lui transmettre cet événement.
