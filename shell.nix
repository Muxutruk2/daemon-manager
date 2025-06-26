{ pkgs ? import <nixpkgs> { } }:

pkgs.mkShell { buildInputs = [ pkgs.bacon pkgs.caddy pkgs.superhtml ]; }
