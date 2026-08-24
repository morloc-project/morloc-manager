# Devil's Advocate Memory Index

- [Pliable container env design verdict](project_pliable_container_env.md) — in-container `morloc make` dep install; Option B (materialize-time solve into bind mounts) beats A (named volume) / C (overlay); prefix-lock + shim-coherence are the crux.
- [Container dotfiles seeding verdict](project_container_dotfiles_seeding.md) — PS1/aliases (later .vimrc/.gitconfig) into `<env>/home`; dotfiles-dir (X) beats inline shell_init (Y); seed-if-absent lazily at run time; Y dead-ends at full-dotfiles goal.
