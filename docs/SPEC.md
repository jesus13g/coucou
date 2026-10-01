# Coucou — spécification

Toutes les mesures sont en pixels logiques. Les valeurs viennent de `design/prototype/coucou.html` (constantes `NW`, `NH`, `EW`, `VIEWS`, `STATES`, `EMOTES`, `PISTES`, `AGENTS`, classe `Bot`). En cas de doute, relire le code du prototype.

---

## 1. Fenêtre

- Une fenêtre Tauri sans bordure, fond transparent, sans ombre, toujours au premier plan, hors de la barre des tâches et d'Alt-Tab. Elle ne prend jamais le focus au clic (Windows : `WS_EX_NOACTIVATE` + `WS_EX_TOOLWINDOW` ; Linux : fenêtre GTK de type utilitaire, `accept-focus` désactivé).
- Taille fixe 720 × 320, ancrée en haut au centre de l'écran choisi (principal, ou celui sous la souris). L'island est dessinée dedans, collée au bord haut.
- Position (réglages) : par défaut l'island sort du bord haut de l'écran. « Below the top bar » la descend sous une barre ou un panneau du haut qui la couvrirait : hauteur saisie en pixels d'écran, ou détectée (0) — haut de la zone de travail de l'écran (Windows, GNOME, Plasma), sinon l'espace réservé par Hyprland.
- **Clics traversants** : la zone transparente ne doit jamais bloquer les clics. Le passage des clics est basculé à 60 Hz selon que le curseur est dans la forme de l'island (plus 14 px de marge) ou pas.
- La fenêtre peut prendre le focus uniquement quand un champ texte de l'island a le focus (chat, prompt). Sinon elle ne vole jamais le focus.
- Dimensions de base : `wN = 184`, `hN = 32` (valeurs du prototype).
- Au repos (`hidden`), l'island se rétracte à une hauteur nulle dans le bord haut de l'écran ; la fenêtre se réduit à une bande invisible de 240 × 6 qui la réveille au survol.
- Suivi de la souris : polling de la position du curseur à chaque frame (Windows : `GetCursorPos` ; Linux : `XQueryPointer`), arrêté quand l'island est masquée. Aucune permission nécessaire.

### Forme de l'island
- Rectangle noir `#000`, coins hauts carrés (il se fond dans le bord de l'écran), coins bas arrondis : 14 pt en hidden/peek/compact, 30 pt en expanded.
- Deux « oreilles » concaves de 14 pt aux coins hauts, à l'extérieur, pour que la forme coule dans le bord de l'écran (voir `#island::before/::after` du prototype).

## 2. Modes de l'island

| Mode | Largeur | Hauteur | Bonhomme | Agents secondaires |
|---|---|---|---|---|
| `hidden` | wN | hN | invisible | invisibles |
| `peek` | wN + 64 | hN | Ø 18, centre x = 19 | invisibles |
| `compact` | wN + 104 | hN | Ø 20, centre x = 27 | grille 2×2 dans l'oreille droite |
| `expanded` | 640 | selon la vue (§5) | selon la vue | selon la vue |

(Ø = diamètre du corps. Le canvas du personnage fait Ø / 0,6 de côté : le corps occupe 60 % du canvas, le reste sert aux particules, mains et badge.)

Grille compact : pastilles Ø 9,5 autour du point (largeur − 27, hN/2), écart ±6. 1 agent : centré. 2 : côte à côte. 3 : deux en haut, un en bas. 4 : carré.

## 3. Règles de comportement (validées par Louis)

1. **Rien ne tourne** → `hidden`. Totalement invisible.
2. **Souris en haut au centre de l'écran** alors que `hidden` → `peek` immédiatement, le bonhomme sort en faisant coucou (mains + son `peek` + son `greet`). Si la souris reste 650 ms → `expanded` (vue `overview`, ou `empty` s'il n'y a aucune tâche). Si elle part pendant le peek → retour `hidden` après 600 ms.
3. **Des tâches tournent et Louis est actif** → `compact` : très fin, le bonhomme visible, il suit la souris des yeux partout sur l'écran.
4. **Survol en compact** → `expanded` après 200 ms. Clic sur le bonhomme en compact → `expanded` tout de suite.
5. **Fermeture auto** : une fois ouverte, l'island se replie après **60 s sans activité** (mouvement de souris sur l'island, clic, frappe). Quitter l'island ne la ferme pas. Pendant les 10 dernières secondes, un trait de 2 pt en bas au centre (160 pt → 0, blanc 35 %) montre le compte à rebours. `Échap` ferme.
6. **Louis absent** (aucun mouvement de souris depuis 3 min, réglable) → `hidden`, même avec des tâches. Au premier mouvement → retour `compact` si des tâches tournent.
7. **Alertes** (permission, question, erreur) : l'island s'ouvre seule sur la vue de l'alerte, **même si Louis est absent**, et reste ouverte (pas de fermeture auto) jusqu'à sa réponse.
8. **Terminé** : l'island s'ouvre sur la vue `finished` pendant 5,2 s, puis retire la tâche et se replie.
9. Plusieurs alertes en même temps : file d'attente, une à la fois, l'ordre d'arrivée.
10. **Focus** : le gros bonhomme représente la tâche en focus (la dernière alerte, sinon la première qui travaille). Les autres tâches sont les mini-bonhommes. Cliquer un mini-bonhomme le met en focus.

## 4. Animations de l'island

- Ouverture / agrandissement : 520 ms, ressort avec léger dépassement, équivalent `cubic-bezier(.32,1.22,.42,1)`. Ressort de départ : `response 0.5`, `dampingFraction 0.72`, à ajuster à l'œil contre le prototype.
- Fermeture / rétrécissement : 340 ms, `cubic-bezier(.45,0,.2,1)`, sans dépassement.
- Largeur, hauteur, rayon, position et taille du bonhomme, position et taille des mini-bonhommes animent **ensemble** (effet « élément partagé » : les mini-bonhommes passent de la grille aux pastilles puis à la colonne sans disparaître).
- Contenu des vues : sortie 160 ms (opacité 0, flou 8, échelle 0,97) ; entrée 300 ms avec 160 ms de retard (après que le conteneur a commencé à grandir). L'en-tête apparaît avec 300 ms de retard.
- Mini-bonhommes : décalage de 35 ms par index.
- Libellés des pastilles : apparaissent 220 ms après le début du mouvement.
- Au passage en `expanded`, le bonhomme cligne des yeux.
- Sons : `open` à l'ouverture, `close` à la fermeture.

## 5. Vues (mode expanded, largeur 640)

Structure commune : en-tête de 34 pt (onglets à gauche : Vue d'ensemble, Demander, Déposer ; à droite : « N en cours » + bouton son). Contenu inséré de 36 en haut, 10 à gauche, droite, bas. Cartes : rayon 20, fond `#141518`, bord blanc 3,5 %. Dans les vues autres que `overview`, les mini-bonhommes passent en **colonne** à droite (Ø 16, x = largeur − 31, y = 50 + i × 24) et la carte laisse 42 pt à droite.

Voile de couleur des cartes : dégradé radial depuis le bas (120 % × 90 %, centre 50 % / 130 %), couleur de l'état :
rouge `rgba(244,80,94,.55)`, vert `rgba(52,211,153,.5)`, rose `rgba(244,114,182,.55)`, ambre `rgba(245,165,36,.42)`, cyan `rgba(34,211,238,.38)`, indigo `rgba(99,102,241,.5)`, neutre `rgba(255,255,255,.08)`.

| Vue | Hauteur | Bonhomme (x, Ø) | Contenu | Capture |
|---|---|---|---|---|
| `overview` | 196 | 64, 70 | carte gauche 322 de large : ligne agent + défilé de tâches ; carte droite : pastilles | 03 |
| `empty` | 150 | 70, 62 | « Rien ne tourne pour l'instant. » + bouton « Demander à Claude » | 16 |
| `approval` | 206 | 62, 56 | agent + « Claude Code veut lancer une commande », bloc code, Refuser (N), Toujours autoriser, Autoriser (Y) | 04 |
| `question` | 196 | 62, 56 | agent + question + options en boutons | 05 |
| `error` | 190 | 62, 58 | agent + outil, titre, détail en rouge `#FF8D97`, Relancer, Voir le terminal | 06 |
| `finished` | 170 | 62, 58 | agent + résumé, Voir le terminal, OK | 07 |
| `confused` | 160 | 76, 66 | « Trop de claques d'un coup. » | 08 |
| `upload` | 176 | 140, 62 | zone pointillée, « Dépose tes fichiers ici », étiquettes | 09 |
| `uploading` | 150 | sur la barre, Ø 28 | « Envoi de fichier » + %, barre verte, le bonhomme est le curseur de la barre | 10 |
| `choose` | 170 | 60, 52 | « fichier est prêt. », Poser une question dessus, Envoyer par mail | 11 |
| `mail` | 210 | 56, 46 | champs À, Objet (+ Message optionnel), Envoyer, Annuler | 12 |
| `prompt` | 156 | 52, 44 | pastille de contexte + champ + micro + envoyer | 13 |
| `searching` | 156 | 52, 44 | contexte + texte scintillant « Claude lit la page et cherche sur le web… » | 14 |
| `result` | 262 (s'adapte au contenu, max 320) | 52, 44 | titre, 3 lignes de résultat, boutons | 15 |
| `note` | 136 | 60, 50 | message court (mail envoyé, copié…), se ferme seul après 2 s | — |

Centre vertical du bonhomme : 36 + (hauteur − 46) / 2, sauf `result` (y = 86).

### Défilé de tâches (overview)
- Position x = 112 dans la carte, fenêtre de 96 pt avec masque dégradé haut/bas, 4 lignes de 30 pt (précédente, courante, suivante, suivante+1).
- Ligne courante : 14 pt medium, texte scintillant (dégradé gris → blanc → gris qui balaie en 2,2 s). Autres : 13 pt `#5F646D`, icône 14 pt.
- Toutes les 2,8 s, si la tâche en focus travaille : tout monte de 30 pt en 450 ms `cubic-bezier(.3,.9,.3,1)`.
- Dans l'app réelle, les lignes = les dernières actions de la session (outil + cible : « Edit invoice.ts », « Bash npm test »).

### Pastilles (overview)
- 132 × 34, rayon 17, fond couleur de l'agent à 13 %, bord à 32 %, mini-bonhomme Ø 24 centré à 17 pt du bord gauche, libellé 12 pt couleur de l'agent éclaircie de 25 %. Deux colonnes, écart 8, centrées verticalement dans la carte droite (qui commence à x = 342).

### Boutons
- Pilule, 12,5 pt medium, fond blanc 9 % (survol 15 %), primaire : fond `#F5F6F8` texte `#0B0C0E`. Appui : échelle 0,94. Raccourcis affichés en petite pastille bordée (Y, N).

## 6. Couleurs des agents (fixes)

| Agent | Couleur |
|---|---|
| Korus | `#FF6B5B` |
| SBE Hub | `#2DD4A7` |
| Morning AI Brief | `#F7B32B` |
| Publication IG | `#A78BFA` |
| louisraille.fr | `#38BDF8` |
| Autres | prendre dans cet ordre : `#F472B6`, `#34D399`, `#FB923C`, `#60A5FA`, `#E879F9`, puis boucler |

Nom d'une session Claude Code = nom du dossier de travail (`cwd`), avec une table d'alias réglable (ex. `sbe-hub` → « SBE Hub »).

## 7. Le personnage : Mochi

Porter la classe `Bot` du prototype **telle quelle** en Canvas 2D (`windows/src/mochi/engine.ts`), animée par `requestAnimationFrame` et en pause quand l'island est masquée. Constantes Mochi (`PISTES.mochi`) :

- R = 0,3 × côté du canvas. Corps : superellipse d'exposant 2,7, rayons rx = 1,14 R, ry = 0,88 R, décalé de +0,06 R vers le bas.
- Dégradé du corps : `#FFFAF5` (haut droite) → `#DDCCBF` (bas gauche). Teinte d'état : dégradé linéaire de bas en haut, couleur d'état à 92 % × tint jusqu'à transparent à −0,25 ry. Ombrage radial (bord 20 % noir) et reflet radial blanc 55 % en haut à droite.
- Joues : deux ellipses rose `rgba(255,120,150,.5 × blush)`, blush minimum 0,35 pour Mochi, suivent le regard.
- Yeux : encre `#1A1412`, largeur 0,25 R, hauteur 0,27 R, écart angulaire ±0,37 rad, inclinaison verticale −0,12 rad. Projection sur une sphère (yaw, pitch, roll) avec raccourci de perspective et découpe par la silhouette : c'est ce qui donne les roulades (les yeux sortent par le haut et reviennent par le bas).
- Regard : suit la souris avec retard (`tanh(dx/260)`, `tanh(dy/200)`, lissage exponentiel). Clignement aléatoire toutes les 2,2 à 5,4 s, double clignement 22 % du temps.
- Mini-bonhommes : même moteur, corps teinté de la couleur de l'agent, badges réduits.
- Canvas du gros bonhomme : 230 px × `devicePixelRatio` ; mini : 76 px × `devicePixelRatio`.

### États (`STATES`)

| Clé | Libellé | Couleur | Teinte | Yeux | Badge | Particularité |
|---|---|---|---|---|---|---|
| `idle` | Au repos | `#E6E9EE` | 0 | pilule | aucun | |
| `working` | Travaille | `#3B9EFF` | 0,72 | pilule | pilule « ••• » animée | |
| `thinking` | Réfléchit | `#8B5CF6` | 0,72 | pilule | « ••• » | regarde en haut à droite |
| `searching` | Cherche | `#6366F1` | 0,72 | pilule | « ••• » | yeux qui balaient de gauche à droite |
| `approval` | Attend ton feu vert | `#F5A524` | 0,78 | grands | « ! » | petits sauts en boucle |
| `question` | Pose une question | `#22D3EE` | 0,75 | pilule | « ? » | tête penchée 0,17 rad |
| `error` | Erreur | `#F4505E` | 0,78 | plats | point rouge | secousse horizontale à l'entrée |
| `finished` | Terminé | `#34D399` | 0,35 | contents (arc) | point vert | roulade complète 950 ms + étincelles |
| `ratelimit` | Limite atteinte | `#FB923C` | 0,72 | fatigués | point orange | gouttes de sueur |
| `sleeping` | Dort | `#94A3B8` | 0,32 | fermés | aucun | respiration, « z » qui montent |
| `dizzy` | Sonné | `#F472B6` | 0,7 | spirales | aucun | double roulade 1,3 s |

Halo derrière le bonhomme : dégradé radial couleur de l'état, opacité 0,2 à 0,6 selon l'état (`glow`, `go`), flou 6.

Correspondance avec les vrais événements : voir `INTEGRATIONS.md`. `sleeping` = aucune tâche depuis 10 min et island ouverte manuellement ; `ratelimit` = limite d'usage signalée par Claude Code.

### Émotes (`EMOTES`) et déclencheurs réels

| Émote | Yeux | Extra | Son | Déclencheur |
|---|---|---|---|---|
| Amour | cœurs `#FF4D6D` | joues à fond, cœurs qui montent | `love` | souris immobile 1,9 s sur le bonhomme |
| Surpris | petits points | saut + yeux agrandis | `pop` | quand on l'attrape |
| Fier | étoiles `#F7B32B` | étoiles, tête en arrière | `proud` | résultat de recherche affiché |
| Clin d'œil | un œil fermé | tête penchée | `wink` | mail envoyé, fenêtre attrapée |
| Bâille | fatigués puis fermés | étirement vertical, « z » | `yawn` | juste avant de passer en `sleeping` |
| Content | arcs | joues | — | après une décision, un fichier avalé |
| Agacé | fentes inclinées | halo violet `#A855F7` | `annoyed` | une claque |

## 8. Interactions avec le bonhomme

- **Survol** (expanded) : clignement, yeux ×1,08, son `hover`. Immobile 1,9 s → Amour.
- **Clic** en compact/peek → ouvre. **Clic** en expanded → claque : écrasement (70/130/170 ms), Agacé 800 ms, halo violet, sons `slap` + `annoyed`.
- **3 clics en moins de 1,7 s** → état `dizzy` pendant 3,3 s, vue `confused`, son `dizzy`, puis retour à la vue et à l'état d'avant.
- **Glisser** le bonhomme (> 7 pt) : un bonhomme flottant Ø 54 suit le curseur (Surpris + `pop`), celui de l'island disparaît. Lâché → revient dans l'island en 420 ms en rétrécissant. (L'attache à une autre fenêtre, INTEGRATIONS §4, n'existe pas dans cette version.)
- **Glisser un fichier** depuis le gestionnaire de fichiers vers la zone de l'island (±220 pt autour du centre, jusqu'à 26 pt sous l'island) → vue `upload`, le bonhomme se transforme en « bac » (morph 380 ms avec rebond) et regarde le fichier. Contour vert et voile vert quand le fichier est au-dessus.
- **Déposer** : le fichier file dans le bonhomme (360 ms), `gulp` à 330 ms, écrasement + Content, retour à la forme ronde à 950 ms, vue `uploading` (1,2 à 2,1 s, `tick` tous les 10 %, correspond à la copie dans le dossier de travail de l'app), son `approve`, puis vue `choose`.
- Plusieurs fichiers : même flux, libellé « 3 fichiers ».

## 9. Sons

Fichiers `assets/sounds/*.wav` (48 kHz stéréo), rendus depuis le moteur du prototype avec un gain ×6. **Volume par défaut du lecteur : 0,12** pour retrouver le niveau du prototype ; le curseur de volume des réglages va de 0 à 0,2. Jouer avec `AVAudioPlayer` préchargés (latence nulle), plusieurs sons peuvent se superposer. Désactivable dans l'en-tête de l'island et dans les réglages (persisté).

| Événement | Son |
|---|---|
| peek / coucou | `peek` + `greet` |
| ouverture / fermeture | `open` / `close` |
| survol du bonhomme / petit clic UI | `hover` / `blip` |
| claque / agacé / sonné | `slap` / `annoyed` / `dizzy` |
| travaille / réfléchit / cherche | `work` / `think` / `search` |
| permission / question / erreur / limite | `approval` / `question` / `error` / `rate` |
| terminé | `finish` |
| décision validée, upload fini | `approve` |
| fichier avalé / progression | `gulp` / `tick` |
| envoi (prompt, mail) / attache fenêtre | `send` / `attach` |
| émotes | `love`, `pop`, `proud`, `wink`, `yawn`, `sleep` |

Pas de son pour les mises à jour silencieuses (défilé de tâches, mini-bonhommes qui changent d'état sauf alerte).

## 10. Icône de la zone de notification et réglages

Icône dans la zone de notification (Windows) ou la barre d'état (Linux, AppIndicator). Menu : Ouvrir Coucou, Réglages…, Pause, Quitter.

Fenêtre Réglages (simple) :
- Clé API Anthropic (magasin de clés du système), modèle (par défaut voir INTEGRATIONS §4).
- Claude Code : état des hooks (installés / non), bouton Installer / Désinstaller, délai d'attente d'une décision (défaut 110 s).
- Son on/off, volume. Fermeture auto (défaut 60 s). Délai d'absence (défaut 3 min).
- Lancer au démarrage (plugin autostart de Tauri).
- Alias de noms de projets et couleurs.

## 11. Critères d'acceptation

- Côte à côte avec le prototype, Louis ne voit pas de différence sur le personnage, les couleurs, les timings et les sons.
- Aucun clic perdu à cause de la fenêtre transparente.
- Une session Claude Code n'est jamais bloquée par l'app (app fermée, plantée ou lente → le terminal prend le relais).
- Hidden = 0 % CPU ; compact < 3 % ; mémoire < 100 Mo.
