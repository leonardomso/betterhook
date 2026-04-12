# Homebrew formula for betterhook.
#
# This formula downloads pre-built binaries from GitHub Releases.
# It's meant to live in a separate homebrew-tap repo eventually;
# it's scaffolded here for review and testing.
#
# Usage (once published to a tap):
#   brew tap leonardomso/betterhook
#   brew install betterhook

class Betterhook < Formula
  desc "Memory-efficient, worktree-native git hooks manager built for the AI agent era"
  homepage "https://github.com/leonardomso/betterhook"
  license "MIT"
  version "0.0.2"

  on_macos do
    on_arm do
      url "https://github.com/leonardomso/betterhook/releases/download/v#{version}/betterhook-aarch64-apple-darwin"
      sha256 "UPDATE_ON_RELEASE"
    end
    on_intel do
      url "https://github.com/leonardomso/betterhook/releases/download/v#{version}/betterhook-x86_64-apple-darwin"
      sha256 "UPDATE_ON_RELEASE"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/leonardomso/betterhook/releases/download/v#{version}/betterhook-aarch64-unknown-linux-gnu"
      sha256 "UPDATE_ON_RELEASE"
    end
    on_intel do
      url "https://github.com/leonardomso/betterhook/releases/download/v#{version}/betterhook-x86_64-unknown-linux-gnu"
      sha256 "UPDATE_ON_RELEASE"
    end
  end

  def install
    bin.install stable.url.split("/").last => "betterhook"
  end

  test do
    assert_match "betterhook #{version}", shell_output("#{bin}/betterhook --version")
  end
end
