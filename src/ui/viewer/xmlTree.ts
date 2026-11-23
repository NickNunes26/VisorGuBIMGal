import {
  child,
  children,
  childText,
  escapeXml,
  leaf,
  parseXml as parseXmlCore,
  unescapeXml,
  writeXml,
  XmlError,
  type XmlErrorCode,
  type XmlNode,
  type XmlOut
} from '../../domain/xml/xmlTree';
import { t } from '../i18n';

export {
  child,
  children,
  childText,
  escapeXml,
  leaf,
  unescapeXml,
  writeXml,
  XmlError,
  type XmlErrorCode,
  type XmlNode,
  type XmlOut
};

/** The document element. Throws localized `XmlError` on a document whose tags do not nest. */
export function parseXml(xml: string): XmlNode {
  try {
    return parseXmlCore(xml);
  } catch (err) {
    if (err instanceof XmlError) {
      if (err.code === 'unexpected-closing-tag') {
        throw new XmlError(t('xml.unexpectedClosingTag', { tag: err.tag ?? '' }), err.code, err.tag);
      }
      if (err.code === 'missing-closing-tag') {
        throw new XmlError(t('xml.missingClosingTag', { tag: err.tag ?? '' }), err.code, err.tag);
      }
      throw new XmlError(t('xml.invalidRoot'), err.code, err.tag);
    }
    throw err;
  }
}
