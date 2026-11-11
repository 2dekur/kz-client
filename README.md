# KZ Client

Launcher Minecraft: Java Edition — Tauri v2 (Rust) + React / Tailwind / Framer Motion / skinview3d.

## Arborescence

```
KZ CLIENT/
├── index.html
├── package.json · vite.config.ts · tsconfig.json
├── tailwind.config.js · postcss.config.js
├── app-icon.svg                    source des icônes (npx tauri icon app-icon.svg -o src-tauri/icons)
├── .env.example                    KZ_AZURE_CLIENT_ID, KZ_DISCORD_APP_ID
├── src/                            frontend
│   ├── main.tsx
│   ├── App.tsx                     layout, état global, abonnements aux évènements backend
│   ├── index.css                   thème dark glassmorphism (#09090b, backdrop-blur-2xl, border-white/10)
│   ├── lib/api.ts                  types + wrappers invoke/listen, proxy skins, avatar 2D
│   └── components/
│       ├── AccountPanel.tsx        multi-comptes (avatar, bascule, suppression, ajout Microsoft)
│       ├── Avatar.tsx              tête pixel-art générée depuis la texture
│       ├── ConfigPanel.tsx         version (release/snapshot/beta/alpha), loader, KZ Engine, RAM
│       ├── SkinViewer.tsx          skin 3D skinview3d (idle / marche / course, rotation 360°)
│       ├── PlayButton.tsx          bouton JOUER + états dynamiques
│       ├── ProgressHUD.tsx         barre à ressort (rafraîchie à la fréquence de l'écran), Mo/s, fichiers restants
│       └── ConsoleDrawer.tsx       stdout/stderr du jeu en temps réel
└── src-tauri/
    ├── Cargo.toml · build.rs · tauri.conf.json
    ├── capabilities/default.json
    ├── icons/
    └── src/
        ├── main.rs
        ├── lib.rs                  état, commandes Tauri, refresh auth en arrière-plan
        ├── error.rs                KzError (thiserror) sérialisée vers l'UI
        ├── paths.rs                arborescence AppData, anti path-traversal, écriture atomique
        ├── auth.rs                 OAuth2 PKCE → XBL → XSTS → Minecraft, entitlements, keyring
        ├── versions.rs             manifeste Mojang, profils, règles OS, fusion inheritsFrom, Fabric/Quilt
        ├── forge.rs                installeurs Forge/NeoForge (processeurs modernes + legacy ≤ 1.12)
        ├── kzengine.rs             injection Sodium/Iris/Lithium/FerriteCore/ModMenu/Embeddium via Modrinth
        ├── java.rs                 runtimes Java officiels Mojang (8 / 17 / 21+)
        ├── download.rs             téléchargements parallèles HTTP/2, retries, SHA-1, progression
        ├── launcher.rs             orchestration, natives, classpath, flags JVM, spawn + logs
        └── discord.rs              Rich Presence (thread dédié, reconnexion auto)
```

## Prérequis

1. **Rust** stable ≥ 1.77 : <https://rustup.rs>. Windows : Build Tools Visual Studio (C++) + WebView2 (déjà présent sur Windows 11).
   Linux : `libwebkit2gtk-4.1-dev libdbus-1-dev` (+ dépendances Tauri habituelles).
2. **Node.js** ≥ 18.

## Configuration obligatoire : application Azure

Minecraft n'accepte que des Client ID Azure approuvés par Mojang.

1. <https://portal.azure.com> → *App registrations* → *New registration*.
   - Type de compte : **Personal Microsoft accounts only**.
   - Redirect URI : plateforme **Mobile and desktop applications**, valeur `http://localhost`.
2. *Authentication* → **Allow public client flows : Yes**.
3. Demander l'accès à l'API Minecraft pour ce Client ID : <https://aka.ms/mce-reviewappid>.
   Sans validation, la connexion échoue à l'étape Minecraft (HTTP 403, message explicite dans l'UI).
4. Exporter le Client ID avant de lancer :

```bash
export KZ_AZURE_CLIENT_ID="00000000-0000-0000-0000-000000000000"
```

PowerShell : `$env:KZ_AZURE_CLIENT_ID = "..."`.

Ordre de lecture du Client ID :
1. variable d'environnement `KZ_AZURE_CLIENT_ID` à l'exécution ;
2. fichier `azure-client-id.txt` (une ligne : l'ID) dans le dossier de données, par exemple `%APPDATA%\com.kzclient.launcher\azure-client-id.txt` sous Windows, puis redémarrer l'app ;
3. valeur figée dans le binaire si `KZ_AZURE_CLIENT_ID` est défini pendant `npm run tauri build`.

### Discord Rich Presence (optionnel)

Créer une application sur <https://discord.com/developers/applications>, téléverser une image d'asset nommée `kz_logo`, puis exporter `KZ_DISCORD_APP_ID`. Sans cette variable, le Rich Presence est simplement désactivé.

## Lancer

```bash
npm install
```

```bash
npm run tauri dev
```

```bash
npm run tauri build
```

Tests unitaires Rust :

```bash
cd src-tauri && cargo test
```

## Fonctionnement

| Étape | Détail |
|---|---|
| Auth | Serveur TCP local `127.0.0.1:<port dynamique>`, PKCE S256, `state` anti-CSRF, timeout 5 min. Refresh token → trousseau OS (`keyring`), jamais sur disque. Token Minecraft en mémoire, rafraîchi en silence (démarrage + toutes les 15 min). |
| Possession | `/entitlements/mcstore` + `/minecraft/profile` (les comptes Game Pass ont un profil sans entitlement mcstore). |
| Versions | `version_manifest_v2.json` (cache hors ligne), profil vanilla vérifié SHA-1 contre le manifeste. |
| Loaders | Fabric (`meta.fabricmc.net`), Quilt (`meta.quiltmc.org`) : profil JSON fusionné. Forge / NeoForge : installeur officiel, processeurs client exécutés une fois (marqueur `.kz-installed`), format legacy ≤ 1.12 géré. |
| Java | Runtime Mojang sélectionné par `javaVersion.component` (`jre-legacy`, `java-runtime-gamma`, `-delta`…), fichiers vérifiés SHA-1. |
| Téléchargements | 48 transferts concurrents, HTTP/2, 4 tentatives avec backoff, écriture `.kzpart` puis rename. Vérification SHA-1 complète ; aux lancements suivants avec la même liste, contrôle rapide (présence + taille). |
| Natives | Ancien format (< 1.19) extrait dans `natives/<version>` ; format LWJGL 3.3+ laissé sur le classpath. |
| JVM | `-Xms = -Xmx` = RAM choisie (bornée à RAM totale − 1 Go). Java 21+ et ≥ 6 Go : ZGC générationnel. Sinon G1 réglé basse latence. |
| KZ Engine | Modrinth : dernière version *release* compatible loader + version MC, dépendances obligatoires résolues, SHA-1 vérifié. Les mods ajoutés par le joueur (détectés par hash) ne sont jamais dupliqués. Désactiver le toggle retire uniquement les fichiers injectés. |
| Logs | stdout/stderr relayés ligne par ligne ; les évènements XML log4j de Mojang sont convertis en lignes lisibles avec niveau. |

Données : `%APPDATA%\com.kzclient.launcher\` (Windows), `~/Library/Application Support/com.kzclient.launcher/` (macOS), `~/.local/share/com.kzclient.launcher/` (Linux). Une instance (dossier de jeu) par couple version + loader : `instances/<version>-<loader>/`.
