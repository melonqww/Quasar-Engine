-- Stage 0 gameplay probe. The host exposes only these narrow actions.
function on_interact()
    door.open()
    audio.play("door")
end
