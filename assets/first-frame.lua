local function first_frame()
    if not mp.get_property_native("vo-configured") then return end
    local file = io.open("first-frame", "wb")
    if file then
        file:write(mp.get_property_native("current-tracks/video/image") and "still" or "video")
        file:close()
        mp.unregister_event(first_frame)
    end
end

mp.register_event("playback-restart", first_frame)
