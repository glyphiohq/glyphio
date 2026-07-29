# Homebrew cask for Glyphio.
#
# This file is the source of truth; the published copy lives in the tap repo
# (glyphiohq/homebrew-tap) as Casks/glyphio.rb. After cutting a release:
#
#   scripts/update-cask.sh <version>   # rewrites version + sha256 below
#   cp packaging/homebrew/glyphio.rb ../homebrew-tap/Casks/glyphio.rb
#
# Why a tap matters while Glyphio is unsigned: Homebrew quarantines casks by default like any
# other download, but it also supports `--no-quarantine`, which is a single documented flag
# rather than a trip through System Settings. See docs/INSTALL.md.
cask "glyphio" do
  version "1.0.0"
  sha256 "60267e46c8f69071258ee99e6b5d89f1234b945975ad6ab6ae04192f978ba13c"

  url "https://github.com/glyphiohq/glyphio/releases/download/v#{version}/Glyphio_#{version}_aarch64.dmg"
  name "Glyphio"
  desc "Local-first text expansion and screenshot capture with self-hostable team sync"
  homepage "https://github.com/glyphiohq/glyphio"

  depends_on macos: ">= :sonoma" # matches bundle.macOS.minimumSystemVersion (14.0)
  depends_on arch: :arm64

  app "Glyphio.app"

  # Glyphio keeps snippets, capture history and settings here. `brew uninstall` leaves them
  # alone; `brew uninstall --zap` is the "remove everything" the user has to ask for.
  zap trash: [
    "~/Library/Application Support/Glyphio",
    "~/Library/Logs/Glyphio",
    "~/Library/Preferences/io.glyphio.app.plist",
    "~/Library/Saved Application State/io.glyphio.app.savedState",
  ]

  caveats <<~EOS
    Glyphio is not yet notarized by Apple (a Developer ID costs $99/year and the project is
    donation-funded). Installing with --no-quarantine skips the Gatekeeper warning:

      brew install --cask --no-quarantine glyphiohq/tap/glyphio

    Glyphio needs two macOS permissions, and will walk you through both on first run:
      * Accessibility  — to type expansions into other apps
      * Screen Recording — to capture the screen
  EOS
end
