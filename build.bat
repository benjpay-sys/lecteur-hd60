@echo off
rem Compile lecteur-hd60 en un seul .exe autonome et le copie dans le dossier dist
where cargo >nul 2>nul || (echo Rust n est pas installe : https://rustup.rs & pause & exit /b 1)
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc || (echo Echec de la compilation & pause & exit /b 1)
if not exist dist mkdir dist
copy /Y target\x86_64-pc-windows-msvc\release\lecteur-hd60.exe dist\lecteur-hd60.exe
echo.
echo Termine : dist\lecteur-hd60.exe (fichier unique, sans dependance)
pause
