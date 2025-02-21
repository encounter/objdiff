import type { WasiLoggingLogging010Draft as logging } from '../pkg/objdiff';

export const log: typeof logging.log = (level, context, message) => {
  const msg = `[${context}] ${message}`;
  switch (level) {
    case 'trace':
      console.trace(msg);
      break;
    case 'debug':
      console.debug(msg);
      break;
    case 'info':
      console.info(msg);
      break;
    case 'warn':
      console.warn(msg);
      break;
    case 'error':
      console.error(msg);
      break;
    case 'critical':
      console.error(msg);
      break;
  }
};
