package dev.flm.client

/**
 * Depacketiza RTP/H.264 (RFC 6184) mínimo: pacotes de NAL único e
 * fragmentação FU-A (tipo 28). STAP-A não é usado pelo rtph264pay do
 * GStreamer no modo atual, então não é suportado aqui.
 *
 * Emite unidades NAL completas já prefixadas com start code Annex-B
 * (00 00 00 01), prontas para alimentar o MediaCodec.
 */
class RtpH264Depacketizer {

    private var fuBuffer: ByteArray? = null
    private var fuLength = 0

    /** Retorna a NAL Annex-B completa, ou null se o pacote for parte de uma fragmentação ainda incompleta. */
    fun depacketize(packet: ByteArray, packetLength: Int): ByteArray? {
        if (packetLength < 12) return null

        val firstByte = packet[0].toInt()
        val csrcCount = firstByte and 0x0F
        val hasExtension = (firstByte and 0x10) != 0

        var offset = 12 + csrcCount * 4
        if (hasExtension) {
            if (offset + 4 > packetLength) return null
            val extLenWords = ((packet[offset + 2].toInt() and 0xFF) shl 8) or
                (packet[offset + 3].toInt() and 0xFF)
            offset += 4 + extLenWords * 4
        }
        if (offset >= packetLength) return null

        val nalHeader = packet[offset].toInt() and 0xFF
        val nalType = nalHeader and 0x1F

        return when {
            nalType in 1..23 -> {
                val nalSize = packetLength - offset
                annexB(packet, offset, nalSize)
            }
            nalType == 28 -> depacketizeFuA(packet, offset, packetLength)
            else -> null // STAP-A/B, MTAP, etc. -- não usados pelo encoder atual
        }
    }

    private fun depacketizeFuA(packet: ByteArray, offset: Int, packetLength: Int): ByteArray? {
        if (offset + 2 > packetLength) return null
        val fuIndicator = packet[offset].toInt() and 0xFF
        val fuHeader = packet[offset + 1].toInt() and 0xFF
        val start = (fuHeader and 0x80) != 0
        val end = (fuHeader and 0x40) != 0
        val originalNalType = fuHeader and 0x1F
        val payloadOffset = offset + 2
        val payloadSize = packetLength - payloadOffset
        if (payloadSize <= 0) return null

        if (start) {
            val reconstructedHeader = (fuIndicator and 0xE0) or originalNalType
            val buf = ByteArray(4 + 1 + payloadSize)
            buf[0] = 0; buf[1] = 0; buf[2] = 0; buf[3] = 1
            buf[4] = reconstructedHeader.toByte()
            System.arraycopy(packet, payloadOffset, buf, 5, payloadSize)
            fuBuffer = buf
            fuLength = buf.size
            return null
        }

        val current = fuBuffer ?: return null // fragmento de continuação sem início conhecido
        val needed = fuLength + payloadSize
        val grown = if (current.size >= needed) current else current.copyOf(needed)
        System.arraycopy(packet, payloadOffset, grown, fuLength, payloadSize)
        fuLength = needed
        fuBuffer = grown

        return if (end) {
            val result = grown.copyOf(fuLength)
            fuBuffer = null
            fuLength = 0
            result
        } else {
            null
        }
    }

    private fun annexB(packet: ByteArray, offset: Int, size: Int): ByteArray {
        val out = ByteArray(4 + size)
        out[0] = 0; out[1] = 0; out[2] = 0; out[3] = 1
        System.arraycopy(packet, offset, out, 4, size)
        return out
    }
}
