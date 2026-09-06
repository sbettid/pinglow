{{- define "pinglow.timescaledb.fullname" -}}
{{- printf "%s-timescaledb" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "pinglow.timescaledb.secretName" -}}
{{- default "pinglow-db-credentials" .Values.timescaledb.secretName -}}
{{- end -}}

{{- define "pinglow.redis.secretName" -}}
{{- default "pinglow-redis-password" .Values.redis.secretName -}}
{{- end -}}

{{- define "pinglow.db.secretName" -}}
{{- default "pinglow-db-credentials" .Values.pinglow.db.secretName -}}
{{- end -}}