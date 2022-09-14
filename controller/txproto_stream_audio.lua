audio_monitor_id = nil

function io_update_cb(identifier, entry)
    print("Add entry", entry.name)

    if audio_monitor_id == nil and entry.type == "monitor" and entry.default then
        print("Use audio monitor", entry.name)
        audio_monitor_id = identifier
    end
end

function muxer_stats(stats)
    statusline = "Encoding, bitrate: " .. math.floor(stats.bitrate / 1000) .. " Kbps"
    tx.set_status(statusline)
end

function main(...)
    local arg = {...}
    rtp_host, rtp_port = arg[1], arg[2]

    event = tx.register_io_cb(io_update_cb)
    event.destroy()

    tx.set_epoch(0)

    source_audio = tx.create_io(audio_monitor_id, {
        buffer_ms = 20,
    })

    encoder = tx.create_encoder({
        encoder = "libopus",
        options = {
            b = 10^3 * 128,
            application = "audio",
            frame_duration = 20,
            vbr = "on",
        },
    })
    encoder.link(source_audio)

    muxer_a = tx.create_muxer({
        out_url = string.format("rtp://%s:%s", rtp_host, rtp_port),
        out_format = "rtp",
        options = {
            max_interleave_delta = 0,
        },
        priv_options = {
            low_latency = true,
        }
    })
    muxer_a.link(encoder)

    tx.commit()
end
