# lecteur-hd60 (image + son, affichage GPU)

Lecteur faible latence pour carte de capture USB (NZXT Signal HD60 ou toute carte UVC).
Fenêtre native, sans OBS : image + son de la carte, pensé pour jouer dessus.
L'affichage et l'agrandissement sont faits par la carte graphique (DirectX 12 / Vulkan).

## Obtenir le .exe autonome (Windows)
Double-cliquez sur `build.bat` : il compile et crée `dist\lecteur-hd60.exe`.
C'est un fichier unique : pas de DLL à copier, pas de Visual C++ Redistributable à installer.
Il n'utilise que des composants de Windows 10/11 (hors éditions « N » sans Media Feature Pack).
Le code utilise AVX2 (processeurs de 2013 et après) et exige une carte graphique compatible DirectX 12 ou Vulkan.
Outils nécessaires à la compilation : Rust (rustup.rs) et « Build Tools for Visual Studio »
(charge de travail « Développement Desktop en C++ »).
Le fichier `Cargo.lock` fixe les versions des bibliothèques : ne le supprimez pas.

## Interface
La fenêtre a deux parties : l'**image** à gauche et un **panneau clair** à droite. Le panneau glisse pour
s'ouvrir ou se ranger, et l'image se place dans la place qu'il laisse libre. Sans signal, l'image est remplacée
par une mire de barres.

### Accueil
Au lancement, la carte s'ouvre et l'**aperçu en direct** s'affiche à gauche (pastille « Aperçu », sans son).
Le panneau montre la carte, le son et le rendu détectés (voyant vert = prêt, orange = à vérifier, rouge = absent),
le traitement de l'image, trois profils d'affichage et le bouton **Lancer le direct** (ou **Entrée**) :
le panneau se range, le son démarre et la pastille passe à « Direct » (voyant rouge).
- **Latence minimale** : affichage immédiat, filtre bilinéaire (déchirement possible).
- **Équilibré** : sans déchirement, Lanczos. C'est le réglage par défaut.
- **Image soignée** : Lanczos synchronisé avec l'écran (un peu plus de retard).
Les profils ne changent que l'affichage ; la capture et le son se règlent dans **Réglages**.

**Traitement de l'image** : *Carte graphique* (par défaut) ou *Processeur*, depuis l'accueil ou les Réglages, à chaud.
Le mode processeur utilise le rendu logiciel de Windows (WARP) : il marche sans carte graphique compatible, mais il
est plus lent. En changeant de matériel, le profil choisi est conservé et adapté (processeur : bilinéaire à la
place de Lanczos, bicubique pour « Image soignée »). Si la carte graphique est inutilisable au lancement, le
programme bascule tout seul sur le processeur. Le décodage de l'image de la carte de capture se fait toujours
sur le processeur. Options : `--gpu`, `--cpu` ; réglage enregistré : `compute=gpu|cpu`.
- Depuis le direct, **H** (ou **Échap**, hors plein écran) revient à l'accueil : le son s'arrête, l'aperçu continue.
- Si la carte ne s'ouvre pas, l'image affiche la cause avec **Réessayer**, **Réglages** et **Accueil**.
- Le bouton **Réglages** (icône de curseurs, rappel de la touche Tab) ouvre le panneau de réglages. Les raccourcis utiles sont toujours rappelés en bas du panneau ; la liste change selon la situation (accueil, direct, réglages ouverts). **Quitter l application** se trouve dans Réglages > Général (ou Échap à l accueil).
- La case « Lancer directement au démarrage » saute l accueil ; l option `--direct`
  fait de même pour une session. Pensez à **Enregistrer les réglages** pour garder la case cochée.

### Réglages
Appuyez sur **Tab** ou **F1** : le panneau affiche les réglages en quatre onglets (Image, Son, Capture, Général),
l'image restant visible à côté. Les mêmes touches (ou **Fermer**) le rangent.
- **Image** : filtre d'agrandissement, netteté, mode d'affichage (Mailbox / vsync / immédiat), plein écran,
  pastille d'infos, taille du menu. Ces réglages s'appliquent tout de suite.
- **Son** : activer/couper le son, choisir l'entrée audio, volume, sourdine. Appliqué tout de suite.
- **Capture** : carte, résolution, images par seconde, format. À valider avec **Appliquer la capture**
  (la capture est relancée sans fermer le programme ; le point ● indique des changements non appliqués).
- **Enregistrer les réglages** : sauvegarde dans `%APPDATA%\lecteur-hd60\reglages.txt` ; ils sont relus au
  prochain lancement. **Valeurs par défaut** rétablit tout (sans enregistrer). Le fichier est facultatif.
- Si la carte ne s'ouvre pas, l'écran affiche la cause avec des boutons pour réessayer.
- Les options de la ligne de commande remplacent les réglages enregistrés pour la session ;
  `--no-config` ignore le fichier enregistré.

## Lancer
    lecteur-hd60.exe                   fenêtre, avec l'écran d'accueil
    lecteur-hd60.exe --direct          lance le direct tout de suite
    lecteur-hd60.exe --fullscreen      plein écran (jouer.bat : plein écran + direct)
    lecteur-hd60.exe --list            liste les périphériques vidéo et audio

## Options
    --list, -l          liste les périphériques vidéo et audio
    --no-config         ignore le fichier de réglages enregistré
    --direct, -g        saute l'écran d'accueil et lance le direct tout de suite
    --device, -d N      index vidéo (auto : nom contenant HD60 / NZXT / Signal)
    --audio, -a N       index de l'entrée audio (auto par défaut)
    --no-audio          désactive le son
    --width, -w N       largeur demandée   (1920)
    --height, -h N      hauteur demandée   (1080)
    --fps, -f N         images/s demandées (60)
    --format auto|mjpeg|yuyv|nv12   format de capture (auto : formats bruts d'abord)
    --fullscreen, -p    démarre en plein écran
    --upscale auto|lanczos|bicubic|bilinear|nearest   agrandissement (auto = lanczos)
    --sharpen 0-100     netteté du filtre bicubique (35)
    --present auto|vsync|immediate
                        auto : sans déchirement, latence minimale (Mailbox)
                        vsync : synchronisé avec l'écran (file classique)
                        immediate : sans synchronisation (déchirement possible)

## Touches
    Entrée                lance le direct (écran d'accueil)
    Tab / F1              menu des réglages
    H                     retour à l'accueil (depuis le direct)
    F / F11 / Alt+Entrée  plein écran (bascule)
    Échap   ferme le menu, puis quitte le plein écran, puis revient à l'accueil, puis quitte · Q quitte toujours
    M sourdine · Haut/Bas volume · I pastille d'infos

## Ce que fait la carte graphique
Le processeur reçoit et décode l'image de la carte de capture, puis l'envoie telle quelle au GPU.
Le GPU fait l'agrandissement (Lanczos 3 par défaut) et l'affichage. Le filtre est calculé en deux passes
(horizontale puis verticale) avec des poids précalculés : environ 12 lectures de texture par pixel, sans calcul de sinus.
- Le titre de la fenêtre indique la résolution, le format, les i/s, le filtre et le mode de présentation.
- Un agrandissement ne rajoute pas de détails : la HD60 capture au maximum en 1080p60.
- `--present auto` utilise Mailbox quand il est disponible : l'image la plus récente est affichée
  à chaque rafraîchissement, sans déchirement et sans file d'attente. Sinon, vsync classique.
- Si l'image est réduite (fenêtre plus petite que la capture), le filtre Lanczos s'adapte automatiquement.

## Si « Impossible d'ouvrir la carte »
Le programme essaie plusieurs formats (YUYV, NV12, MJPEG, automatique) et affiche la cause de chaque échec.
- Fermez NZXT CAM, OBS, Discord, le navigateur et l'appli Caméra de Windows.
- Windows : Paramètres > Confidentialité et sécurité > Caméra > « Autoriser les applications de bureau à accéder à votre caméra ».
- Port USB 3.x direct, sans hub.
- Essayez `--format yuyv` ou `--format nv12`, ou `-w 1280 -h 720`.

## Si le son craque
Essayez une autre entrée avec `--list` puis `--audio N`. Le son passe par le PC : coupez le son de l'écran
si vous l'entendez en double.

## Si l'affichage GPU échoue
Le message « Affichage GPU impossible » indique la cause. Mettez à jour le pilote de la carte graphique.
Pour forcer un autre moteur graphique : variable d'environnement `WGPU_BACKEND=vulkan` (ou `dx12`).

## Tests
    cargo test --release -- --nocapture
Les tests couvrent le shader (agrandissement, bandes noires, réduction, dégradé), le dessin de l'accueil et du menu
(variables `HOME_DUMP` / `MENU_DUMP` : chemin d'un fichier .ppm où enregistrer l'image dessinée) et l'enregistrement des réglages. Ceux qui ont besoin d'un GPU sont ignorés s'il n'y en a pas.

## Logo
Un symbole de lecture construit avec huit lignes de balayage (les lignes d une image vidéo), en blanc sur la tuile bleue de l interface.
Fichiers dans `assets\` :
- `logo.svg` (icône seule) et `logo-horizontal.svg` (icône + nom, en Bahnschrift, bleu #2243D4) : à modifier en premier ;
- `logo-16.png` à `logo-512.png`, `icon.ico` (16 à 256 px) : icône de l'exe, intégrée par `build.rs` et `assets\app.rc` ;
- `icon-64.rgba` : image brute 64x64 utilisée pour l'icône de la fenêtre et de la barre des tâches.
Les PNG, l'ICO et le RGBA sont dessinés à partir des mêmes rectangles que les SVG ; si vous changez le logo,
régénérez-les tous.

## Qualité des couleurs (YUYV)
La carte envoie ses images en YUYV (4:2:2) : la luminosité est complète, mais les couleurs n'ont que la moitié de la
résolution horizontale. Le programme copie ces octets tels quels vers la carte graphique, qui fait la conversion en RVB :
les couleurs des pixels impairs sont interpolées avec celles du voisin (au lieu d'être répétées, ce qui donnait des
bords de couleur « en blocs » sur les petits textes colorés), puis la matrice BT.709 est utilisée à partir de 720 lignes
(BT.601 en dessous). Le processeur ne fait plus que copier : moins de charge et moins de latence. Les formats NV12 et MJPEG
sont encore décodés par le processeur. Les couleurs peuvent paraître très légèrement différentes de l'ancienne version
(BT.709 au lieu de BT.601).

## Licence
MIT : voir le fichier `LICENSE`.

## Exe depuis GitHub
Le dépôt contient un workflow GitHub Actions (`.github\workflows\build.yml`) qui compile l'exe sur Windows à chaque
envoi sur `main` : il est téléchargeable dans l'onglet **Actions** (artefact `lecteur-hd60-windows-x64`, gardé 30 jours).
Pour le publier dans **Releases** : `git tag v0.3.0` puis `git push origin v0.3.0`.
