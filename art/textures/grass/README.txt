Grass tuft cards (toony/anime).
1. Generate raw tufts on white (schnell ignores negatives, so the positive
   prompt forces "floating on seamless white, no ground/shadow"):
   generate.py icon --keep-bg --variants 6 --seed 8200 --size 512 \
     --prompt "a bunch of long grass blades ... floating ... seamless pure white ... no ground, no shadow ..."
2. Cut out: white-key (-fuzz 10% -transparent white) + a saturation gate
   (-colorspace HSL -channel G -separate -level 5%,13%) to drop the faint grey
   contact-shadow residue while keeping the saturated green + tan roots. rembg
   (u2net) left white-box halos on bright-grass-on-white, so the white key is
   used instead (cleaner, no detail loss).
3. trim, -resize 250x500, -gravity south -extent 256x512  -> tuft_{1..6}.png
4. 3x2 +append/-append -> assets/textures/grass_atlas.png (768x1024).
   tuft_4 -> assets/textures/grass_tuft.png (single, for the hay node).
The detail-grass instancer picks a random atlas cell per card (i_b.z); see
src/app/scene/grass/instancing.rs + assets/shaders/grass_instanced.wgsl.
